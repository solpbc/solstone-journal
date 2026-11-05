// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as RoutePath, State};
use axum::http::header::HeaderValue;
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{JournalRoot, day_dirs, day_path};
use solstone_core_journal_stats_cli::load_fresh_day_cache;

use crate::counts_flight::{self, Delivery, Disposition, RequestRole};
use crate::day::prepare_day;
use crate::{AppState, TranscriptError, legacy_error_response};

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

pub(crate) async fn index(
    State(state): State<Arc<AppState>>,
    admitted: Option<Extension<Arc<JournalRoot>>>,
) -> Response {
    let delivery = counts_flight::execute(
        admitted.map(|Extension(r)| r),
        &state.journal_root,
        &state.clock,
        RequestRole::Index,
    )
    .await;
    match delivery {
        Delivery::Ready {
            counts,
            disposition,
        } => {
            let mut response = Json(date_nav_index(&counts)).into_response();
            let val = if disposition == Disposition::Immediate {
                HeaderValue::from_static("coalesced;desc=\"immediate\"")
            } else {
                HeaderValue::from_static("coalesced;desc=\"queued\"")
            };
            response.headers_mut().append(SERVER_TIMING, val);
            response
        }
        Delivery::Failed {
            mut response,
            disposition,
        } => {
            let val = if disposition == Disposition::Immediate {
                HeaderValue::from_static("coalesced;desc=\"immediate\"")
            } else {
                HeaderValue::from_static("coalesced;desc=\"queued\"")
            };
            response.headers_mut().append(SERVER_TIMING, val);
            response
        }
    }
}

pub(crate) async fn stats(
    State(state): State<Arc<AppState>>,
    admitted: Option<Extension<Arc<JournalRoot>>>,
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
    let delivery = counts_flight::execute(
        admitted.map(|Extension(r)| r),
        &state.journal_root,
        &state.clock,
        RequestRole::MonthStats,
    )
    .await;
    match delivery {
        Delivery::Ready {
            counts,
            disposition,
        } => {
            let filtered: BTreeMap<_, _> = counts
                .iter()
                .filter(|(day, count)| day.starts_with(&month) && **count > 0)
                .map(|(day, count)| (day.clone(), *count))
                .collect();
            let mut response = Json(filtered).into_response();
            let val = if disposition == Disposition::Immediate {
                HeaderValue::from_static("coalesced;desc=\"immediate\"")
            } else {
                HeaderValue::from_static("coalesced;desc=\"queued\"")
            };
            response.headers_mut().append(SERVER_TIMING, val);
            response
        }
        Delivery::Failed {
            mut response,
            disposition,
        } => {
            let val = if disposition == Disposition::Immediate {
                HeaderValue::from_static("coalesced;desc=\"immediate\"")
            } else {
                HeaderValue::from_static("coalesced;desc=\"queued\"")
            };
            response.headers_mut().append(SERVER_TIMING, val);
            response
        }
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
    let directory = day_path(root, day, false).map_err(TranscriptError::display)?;
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
