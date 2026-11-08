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
    RangeError, Sources, TalentSource, cluster, cluster_period, cluster_range, cluster_span,
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
    let journal_root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsRead,
        move || {
            let sources = Sources {
                transcripts: query.transcripts.as_deref() == Some("1"),
                percepts: query.percepts.as_deref() == Some("1"),
                talents: if query.agents.as_deref() == Some("1") {
                    TalentSource::All
                } else {
                    TalentSource::Disabled
                },
                voices: None,
                heard_owner: None,
                // The owner's own page keeps showing their workouts.
                strava_tiles: true,
            };
            let markdown =
                if let (Some(start), Some(end)) = (query.start.as_deref(), query.end.as_deref()) {
                    cluster_range(
                        &journal_root,
                        &day,
                        start,
                        end,
                        &sources,
                        query.stream.as_deref(),
                    )
                    .map_err(|error| match error {
                        RangeError::UnknownStream(detail) => invalid(&detail),
                        RangeError::Invalid(_) => error_response(error),
                    })
                } else if let Some(segments) = query.segments.as_deref() {
                    let span = segments
                        .split(',')
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>();
                    match cluster_span(
                        &journal_root,
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
                        &journal_root,
                        &day,
                        segment,
                        &sources,
                        query.stream.as_deref(),
                    )
                    .0)
                } else {
                    Ok(cluster(&journal_root, &day, &sources).0)
                };
            match markdown {
                Ok(markdown) => Json(json!({"markdown":markdown})).into_response(),
                Err(response) => response,
            }
        },
    )
    .await
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{Clock, router_with_delete_window};

    /// A workout is body data that no talent reads, and still the owner's own:
    /// their day page shows it.
    #[tokio::test]
    async fn the_day_page_still_shows_a_strava_workout() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .unwrap();
        let tile = root
            .path()
            .join("chronicle/20260810/import.strava/070000_300");
        fs::create_dir_all(&tile).unwrap();
        fs::write(
            tile.join("workout.json"),
            json!({
                "schema": "solstone.import.strava.tile.v1",
                "activity_id": 9,
                "tile": {"index": 0, "count": 1},
                "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
            })
            .to_string(),
        )
        .unwrap();
        let app = router_with_delete_window(
            root.path().to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 12, 12, 0, 0).unwrap()),
            || axum::response::Response::new(Body::from("shell")),
            Duration::from_secs(600),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/app/transcripts/api/read/20260810?transcripts=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let markdown = body["markdown"].as_str().unwrap();
        assert!(
            markdown.contains("## Strava workout: Morning Run"),
            "{markdown}"
        );
    }
}
