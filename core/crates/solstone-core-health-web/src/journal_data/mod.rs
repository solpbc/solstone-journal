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
use chrono::Local;
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};

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
    let now = Local::now();
    let day_res = query.day.as_deref().map(resolve_day).transpose();
    let day_opt = match day_res {
        Ok(day) => day,
        Err(err) => return error_response(err),
    };
    spawn_blocking_response(OwnerReadRole::HealthSummary, move || {
        let day = day_opt.unwrap_or_else(|| now.date_naive());
        let result = build_health_report(&root, (day, day), now);
        report_response(result)
    })
    .await
}

async fn full(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    summary(root, Query(query)).await
}

async fn for_range(root: PathBuf, Query(query): Query<RangeQuery>) -> Response {
    let now = Local::now();
    let range = match resolve_range(query.day_from.as_deref(), query.day_to.as_deref(), now) {
        Ok(range) => range,
        Err(err) => return error_response(err),
    };
    spawn_blocking_response(OwnerReadRole::HealthRange, move || {
        let result = build_health_report(&root, range, now);
        report_response(result)
    })
    .await
}

async fn pipeline_route(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    let now = Local::now();
    let day_str = match query.day.as_deref() {
        None | Some("") => {
            return error_response(HealthError::MissingRequiredField(
                "day is required".to_owned(),
            ));
        }
        Some(day) => day,
    };
    let day = match pipeline::resolve_pipeline_day(day_str) {
        Ok(day) => day,
        Err(err) => return error_response(err),
    };
    spawn_blocking_response(OwnerReadRole::HealthPipeline, move || {
        let result = pipeline::summarize_pipeline_day(&root, day, now);
        match result {
            Ok(report) => Json(report).into_response(),
            Err(error) => error_response(error),
        }
    })
    .await
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
