// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only journal-data routes behind `solstone call health`.

use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;

mod ledger;
mod pipeline;
mod report;

pub(crate) use report::{HealthError, build_health_report, resolve_day, resolve_range};

#[derive(Debug, Deserialize)]
struct DayQuery {
    day: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    day_from: Option<String>,
    day_to: Option<String>,
}

pub(crate) fn api_router(journal_root: PathBuf) -> Router {
    let summary_root = journal_root.clone();
    let full_root = journal_root.clone();
    let range_root = journal_root.clone();
    Router::new()
        .route(
            "/api/health/summary",
            get(move |query| summary(summary_root.clone(), query)),
        )
        .route(
            "/api/health/full",
            get(move |query| full(full_root.clone(), query)),
        )
        .route(
            "/api/health/range",
            get(move |query| for_range(range_root.clone(), query)),
        )
        .route(
            "/api/health/pipeline",
            get(move |query| pipeline_route(journal_root.clone(), query)),
        )
}

async fn summary(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    let now = Utc::now();
    let result = query
        .day
        .as_deref()
        .map(resolve_day)
        .transpose()
        .map(|day| day.unwrap_or_else(|| now.date_naive()))
        .and_then(|day| build_health_report(&root, (day, day), now));
    report_response(result)
}

async fn full(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    summary(root, Query(query)).await
}

async fn for_range(root: PathBuf, Query(query): Query<RangeQuery>) -> Response {
    let now = Utc::now();
    let result = resolve_range(query.day_from.as_deref(), query.day_to.as_deref(), now)
        .and_then(|range| build_health_report(&root, range, now));
    report_response(result)
}

async fn pipeline_route(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    let now = Utc::now();
    let result = match query.day.as_deref() {
        None | Some("") => Err(HealthError::MissingRequiredField(
            "day is required".to_owned(),
        )),
        Some(day) => pipeline::resolve_pipeline_day(day)
            .and_then(|day| pipeline::summarize_pipeline_day(&root, day, now)),
    };
    match result {
        Ok(report) => Json(report).into_response(),
        Err(error) => error_response(error),
    }
}

fn report_response(result: Result<report::HealthReport, HealthError>) -> Response {
    match result {
        Ok(report) => Json(report).into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: HealthError) -> Response {
    match error {
        HealthError::InvalidRequest(detail) => error_envelope(
            "invalid_request_value",
            "Invalid request value",
            detail,
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        HealthError::MissingRequiredField(detail) => error_envelope(
            "missing_required_field",
            "Missing required field",
            detail,
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        HealthError::Internal { context } => {
            log::warn!("native health journal-data report failed: {context}");
            error_envelope(
                "health_report_failed",
                "Health report failed",
                "health report unavailable",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}
