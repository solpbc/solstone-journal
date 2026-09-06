// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use solstone_core_transcripts::{
    Sources, TalentSource, cluster, cluster_period, cluster_range, cluster_span,
};

use crate::day::valid_day;
use crate::{AppState, legacy_error_response};

#[derive(Default, Deserialize)]
pub(crate) struct ReadQuery {
    start: Option<String>,
    end: Option<String>,
    segment: Option<String>,
    segments: Option<String>,
    stream: Option<String>,
    transcripts: Option<String>,
    percepts: Option<String>,
    agents: Option<String>,
}

pub(crate) async fn api_read(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    if !valid_day(&day) {
        return legacy_error_response(
            "invalid_day",
            "that day couldn't be used.",
            "Day not found",
            StatusCode::NOT_FOUND,
        );
    }
    let sources = Sources {
        transcripts: query.transcripts.as_deref() == Some("1"),
        percepts: query.percepts.as_deref() == Some("1"),
        talents: if query.agents.as_deref() == Some("1") {
            TalentSource::All
        } else {
            TalentSource::Disabled
        },
    };
    let markdown = if let (Some(start), Some(end)) = (query.start.as_deref(), query.end.as_deref())
    {
        cluster_range(&state.journal_root, &day, start, end, &sources).map_err(error_response)
    } else if let Some(segments) = query.segments.as_deref() {
        let span = segments
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        match cluster_span(
            &state.journal_root,
            &day,
            &span,
            &sources,
            query.stream.as_deref(),
        ) {
            Ok((markdown, _)) => Ok(markdown),
            Err(detail) => Err(invalid(&detail)),
        }
    } else if let Some(segment) = query.segment.as_deref() {
        Ok(cluster_period(
            &state.journal_root,
            &day,
            segment,
            &sources,
            query.stream.as_deref(),
        )
        .0)
    } else {
        Ok(cluster(&state.journal_root, &day, &sources).0)
    };
    match markdown {
        Ok(markdown) => Json(json!({"markdown":markdown})).into_response(),
        Err(response) => response,
    }
}

fn invalid(detail: &str) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        "that segment or stream couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn error_response(error: impl std::fmt::Display) -> Response {
    legacy_error_response(
        "internal_error",
        "that request didn't finish.",
        error.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}
