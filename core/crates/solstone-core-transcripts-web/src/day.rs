// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::json;
use solstone_core_system_health::{DaySegment, FilesystemSegmentSource, TimeRange, scan_day};

use crate::attach::{
    TranscriptSegment, attach_think_to_segments, attach_visible_streams_to_ranges,
    normalize_markdown_only_segments,
};
use crate::{AppState, TranscriptError, legacy_error_response};

pub(crate) async fn ranges(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    let now = state.clock.now();
    let prepared = match checked_prepare(&state.journal_root, &day, now) {
        Ok(value) => value,
        Err(response) => return response,
    };
    Json(json!({"audio": attach_visible_streams_to_ranges(&prepared.audio, &prepared.segments, "audio"), "screen": attach_visible_streams_to_ranges(&prepared.screen, &prepared.segments, "screen")})).into_response()
}

pub(crate) async fn segments(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    let now = state.clock.now();
    let prepared = match checked_prepare(&state.journal_root, &day, now) {
        Ok(value) => value,
        Err(response) => return response,
    };
    Json(json!({"segments": prepared.segments})).into_response()
}

pub(crate) async fn day(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    let now = state.clock.now();
    let prepared = match checked_prepare(&state.journal_root, &day, now) {
        Ok(value) => value,
        Err(response) => return response,
    };
    Json(json!({"audio": attach_visible_streams_to_ranges(&prepared.audio, &prepared.segments, "audio"), "screen": attach_visible_streams_to_ranges(&prepared.screen, &prepared.segments, "screen"), "segments": prepared.segments})).into_response()
}

pub(crate) struct PreparedDay {
    pub(crate) audio: Vec<TimeRange>,
    pub(crate) screen: Vec<TimeRange>,
    pub(crate) segments: Vec<TranscriptSegment>,
}

pub(crate) fn prepare_day(
    journal_root: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> Result<PreparedDay, TranscriptError> {
    let (audio, screen, segments) = scan_day(&FilesystemSegmentSource, journal_root, day, now)
        .map_err(TranscriptError::health)?;
    let mut segments = segments.into_iter().map(from_native).collect::<Vec<_>>();
    normalize_markdown_only_segments(journal_root, day, &mut segments);
    attach_think_to_segments(journal_root, day, &mut segments)?;
    Ok(PreparedDay {
        audio,
        screen,
        segments,
    })
}

fn from_native(segment: DaySegment) -> TranscriptSegment {
    TranscriptSegment {
        key: segment.key,
        stream: segment.stream,
        start: segment.start,
        end: segment.end,
        types: segment.types,
        data_state: segment.data_state.0,
        think: None,
    }
}
#[allow(clippy::result_large_err)]
fn checked_prepare(root: &Path, day: &str, now: DateTime<Utc>) -> Result<PreparedDay, Response> {
    if !valid_day(day) {
        return Err(invalid_day());
    }
    prepare_day(root, day, now).map_err(|error| error.response())
}
pub(crate) fn valid_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}
pub(crate) fn invalid_day() -> Response {
    legacy_error_response(
        "invalid_day",
        "that day couldn't be used.",
        "Day not found",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}
