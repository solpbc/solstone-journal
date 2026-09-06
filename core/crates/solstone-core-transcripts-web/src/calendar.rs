// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{day_dirs, day_path};
use solstone_core_journal_stats_cli::load_fresh_day_cache;

use crate::day::prepare_day;
use crate::{AppState, TranscriptError, legacy_error_response};

pub(crate) async fn index(State(state): State<Arc<AppState>>) -> Response {
    let now = state.clock.now();
    match counts(&state.journal_root, now) {
        Ok(value) => Json(date_nav_index(&value)).into_response(),
        Err(error) => error.response(),
    }
}
pub(crate) async fn stats(
    State(state): State<Arc<AppState>>,
    RoutePath(month): RoutePath<String>,
) -> Response {
    if !valid_month(&month) {
        return legacy_error_response(
            "invalid_month",
            "that month couldn't be used.",
            "Invalid month format",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let now = state.clock.now();
    match counts(&state.journal_root, now) {
        Ok(counts) => Json(
            counts
                .into_iter()
                .filter(|(day, count)| day.starts_with(&month) && *count > 0)
                .collect::<BTreeMap<_, _>>(),
        )
        .into_response(),
        Err(error) => error.response(),
    }
}
pub(crate) fn counts(
    root: &Path,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, u64>, TranscriptError> {
    day_dirs(root)
        .map_err(TranscriptError::display)?
        .into_keys()
        .map(|day| Ok((day.clone(), day_range_count(root, &day, now)?)))
        .collect()
}
pub(crate) fn day_range_count(
    root: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> Result<u64, TranscriptError> {
    let directory = day_path(root, Some(day), false).map_err(TranscriptError::display)?;
    if let Some(cache) = load_fresh_day_cache(&directory).map_err(TranscriptError::display)? {
        return Ok(cache.stats.transcript_ranges
            + cache.stats.percept_ranges
            + cache.stats.browser_segments);
    }
    let prepared = prepare_day(root, day, now)?;
    Ok(prepared.audio.len() as u64
        + prepared.screen.len() as u64
        + prepared
            .segments
            .iter()
            .filter(|segment| segment.types.iter().any(|kind| kind == "browser"))
            .count() as u64)
}
fn date_nav_index(counts: &BTreeMap<String, u64>) -> Value {
    let positive = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .collect::<Vec<_>>();
    let mut months = BTreeMap::<String, u64>::new();
    for (day, count) in &positive {
        *months.entry(day[..6].to_owned()).or_default() += **count;
    }
    let coverage = positive
        .first()
        .zip(positive.last())
        .map(|((start, _), (end, _))| json!({"start": start, "end": end}));
    json!({"coverage": coverage, "months": months})
}
fn valid_month(month: &str) -> bool {
    month.len() == 6 && month.bytes().all(|byte| byte.is_ascii_digit())
}
