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

/// `complete: true` covers every change whose stream-marker bump committed before the read, and every change to a live day. A change on an older day whose producer committed but did not bump appears within about an hour of it while the calendar is in use.
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
            freshness,
            disposition,
        } => {
            let mut response = Json(date_nav_index(&counts, freshness)).into_response();
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

/// `complete: true` covers every change whose stream-marker bump committed before the read, and every change to a live day. A change on an older day whose producer committed but did not bump appears within about an hour of it while the calendar is in use.
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
        admitted.as_ref().map(|Extension(r)| Arc::clone(r)),
        &state.journal_root,
        &state.clock,
        RequestRole::MonthStats,
    )
    .await;
    match delivery {
        Delivery::Ready { disposition, .. } => {
            let fold_start = std::time::Instant::now();
            let journal_path = admitted
                .as_ref()
                .map(|Extension(r)| r.canonical_path().to_path_buf())
                .unwrap_or_else(|| (*state.journal_root).clone());
            let clock = state.clock.clone();
            let root = admitted.map(|Extension(r)| r);
            let month = month.clone();
            let mut response = solstone_core_convey_http::owner_read::spawn_blocking_response(
                solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsMonthStats,
                move || {
                    let notify_id = root.as_ref().map(|root| root.identity());
                    let res =
                        crate::inventory::fold_month_days(root, &journal_path, &month, &clock);
                    let notify_id = notify_id.or_else(|| {
                        JournalRoot::open(&journal_path)
                            .ok()
                            .map(|opened| opened.identity())
                    });
                    if let Some(notify_id) = notify_id {
                        crate::inventory::notify_inventory_worker(notify_id);
                    }
                    match res {
                        Ok(counts) => Json(counts).into_response(),
                        Err(err) => TranscriptError::display(err).response(),
                    }
                },
            )
            .await;

            let dur_ms = fold_start.elapsed().as_millis();
            let val = if disposition == Disposition::Immediate {
                HeaderValue::from_static("coalesced;desc=\"immediate\"")
            } else {
                HeaderValue::from_static("coalesced;desc=\"queued\"")
            };
            response.headers_mut().append(SERVER_TIMING, val);
            if let Ok(timing_val) = HeaderValue::from_str(&format!("month-fold;dur={dur_ms}")) {
                response.headers_mut().append(SERVER_TIMING, timing_val);
            }
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
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn counts_by_walk(
    root: &Path,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, u64>, TranscriptError> {
    day_dirs(root)
        .map_err(TranscriptError::display)?
        .into_keys()
        .map(|day| Ok((day.clone(), day_range_count(root, &day, now)?)))
        .collect()
}

#[cfg_attr(not(test), allow(dead_code))]
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
pub(crate) fn date_nav_index(counts: &BTreeMap<String, u64>, freshness: Value) -> Value {
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
    json!({"coverage": coverage, "months": months, "freshness": freshness})
}
fn valid_month(month: &str) -> bool {
    month.len() == 6 && month.bytes().all(|byte| byte.is_ascii_digit())
}
