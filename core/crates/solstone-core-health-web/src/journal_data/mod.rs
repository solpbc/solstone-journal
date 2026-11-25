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
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_convey_http::window_catalog::{ENGLISH, text};

mod pipeline;
pub(crate) mod report;

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
    let now = owner_now(&root);
    let day_res = query.day.as_deref().map(resolve_day).transpose();
    let day_opt = match day_res {
        Ok(day) => day,
        Err(err) => return error_response(err),
    };
    spawn_blocking_response(OwnerReadRole::HealthSummary, move || {
        let day = day_opt.unwrap_or_else(|| now.date());
        let result = build_health_report(&root, (day, day), &now);
        report_response(result)
    })
    .await
}

pub(crate) async fn media_backlog(
    root: PathBuf,
    now: jiff::Timestamp,
    locale: &'static str,
) -> Response {
    spawn_blocking_response(OwnerReadRole::HealthMediaBacklog, move || {
        match report::read_segment_backlog_health(&root, now) {
            Ok(backlog) => Json(serde_json::json!({"segment_backlog":backlog})).into_response(),
            Err(error) => error_response_in(error, locale),
        }
    })
    .await
}

async fn full(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    summary(root, Query(query)).await
}

async fn for_range(root: PathBuf, Query(query): Query<RangeQuery>) -> Response {
    let now = owner_now(&root);
    let range = match resolve_range(query.day_from.as_deref(), query.day_to.as_deref(), &now) {
        Ok(range) => range,
        Err(err) => return error_response(err),
    };
    spawn_blocking_response(OwnerReadRole::HealthRange, move || {
        let result = build_health_report(&root, range, &now);
        report_response(result)
    })
    .await
}

async fn pipeline_route(root: PathBuf, Query(query): Query<DayQuery>) -> Response {
    let now = owner_now(&root);
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
        let result = pipeline::summarize_pipeline_day(&root, day, &now);
        match result {
            Ok(report) => Json(report).into_response(),
            Err(error) => error_response(error),
        }
    })
    .await
}

/// The current instant in the journal owner's zone.
fn owner_now(root: &std::path::Path) -> jiff::ZonedDateTime {
    jiff::Timestamp::now().to_zoned_datetime(solstone_core_journal_config::owner_zone(root))
}

fn report_response(result: Result<report::HealthReport, HealthError>) -> Response {
    match result {
        Ok(report) => Json(report).into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: HealthError) -> Response {
    error_response_in(error, ENGLISH)
}

fn error_response_in(error: HealthError, locale: &str) -> Response {
    match error {
        HealthError::InvalidRequest(detail) => error_envelope(
            "invalid_request_value",
            text(locale, "journal.window.health.report.invalid"),
            detail,
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        HealthError::MissingRequiredField(detail) => error_envelope(
            "missing_required_field",
            text(locale, "journal.window.health.report.missing"),
            detail,
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        HealthError::Internal { context } => {
            log::warn!("native health journal-data report failed: {context}");
            error_envelope(
                "health_report_failed",
                text(locale, "journal.window.status.report.failed"),
                text(locale, "journal.window.status.report.unavailable"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}
