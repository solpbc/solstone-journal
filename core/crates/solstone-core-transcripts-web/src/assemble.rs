// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use solstone_core_format::content::body_hidden_days::read_hidden_days;
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
                // Named as an agent's read names them, through the same voice
                // gate: an agent is this read's usual reader.
                voices: solstone_core_talent_runtime::agent_speakers(&journal_root)
                    .map(|speakers| speakers.voices),
                heard_owner: None,
                // The owner's own page keeps showing their workouts, except on
                // a day they hid: a workout reads only on the day it started.
                // While the hidden days can't be read, the page shows them.
                strava_tiles: read_hidden_days(&journal_root).hides(&day) != Some(true),
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

    /// A workout on a day the owner hid leaves that day's page, and one on a
    /// day they didn't stays.
    #[tokio::test]
    async fn the_day_page_leaves_out_a_workout_on_a_hidden_day() {
        use solstone_core_format::content::body_hidden_days::{
            hidden_days_bytes, hidden_days_path,
        };
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .unwrap();
        for day in ["20260810", "20260811"] {
            let tile = root
                .path()
                .join(format!("chronicle/{day}/import.strava/070000_300"));
            fs::create_dir_all(&tile).unwrap();
            fs::write(
                tile.join("workout.json"),
                json!({
                    "schema": "solstone.import.strava.tile.v1",
                    "activity_id": if day == "20260810" { 9 } else { 10 },
                    "tile": {"index": 0, "count": 1},
                    "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
                })
                .to_string(),
            )
            .unwrap();
        }
        let store = hidden_days_path(root.path());
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::write(
            &store,
            hidden_days_bytes(&std::collections::BTreeSet::from(["20260810".to_owned()])),
        )
        .unwrap();
        let hidden = read_markdown(
            root.path(),
            "/app/transcripts/api/read/20260810?transcripts=1",
        )
        .await;
        assert!(!hidden.contains("Strava workout"), "{hidden}");
        let shown = read_markdown(
            root.path(),
            "/app/transcripts/api/read/20260811?transcripts=1",
        )
        .await;
        assert!(shown.contains("## Strava workout: Morning Run"), "{shown}");
    }

    async fn read_markdown(root: &std::path::Path, uri: &str) -> String {
        let app = router_with_delete_window(
            root.to_path_buf(),
            Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 12, 12, 0, 0).unwrap()),
            || axum::response::Response::new(Body::from("shell")),
            Duration::from_secs(600),
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        body["markdown"].as_str().unwrap().to_owned()
    }

    /// The read names speakers as an agent's read does: the owner's lines by
    /// recognized voice or the owner's own answer, a person by voice, and
    /// everyone else by number.
    #[tokio::test]
    async fn the_read_names_the_owner_and_people_only_by_recognized_voice() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .unwrap();
        for (id, entity) in [
            (
                "olu",
                json!({"id": "olu", "name": "Olu", "type": "Person", "is_principal": true}),
            ),
            (
                "mina",
                json!({"id": "mina", "name": "Mina", "type": "Person"}),
            ),
        ] {
            let dir = root.path().join("entities").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("entity.json"), entity.to_string()).unwrap();
        }
        let segment = root
            .path()
            .join("chronicle/20260810/device_watch-audio/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("audio.jsonl"),
            [
                r#"{"raw":"audio.flac"}"#,
                r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"Heard as mine"}"#,
                r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"Heard as Mina"}"#,
                r#"{"start":"00:00:03","speaker":1,"sentence_id":3,"text":"Answered as mine"}"#,
                r#"{"start":"00:00:04","speaker":2,"sentence_id":4,"text":"Guessed as Mina"}"#,
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            segment.join("talents/speaker_labels.json"),
            json!({"labels": [
                {"sentence_id": 1, "speaker": "olu", "method": "owner_centroid", "confidence": "high", "owner_margin_declined": null},
                {"sentence_id": 2, "speaker": "mina", "method": "acoustic", "confidence": "high"},
                {"sentence_id": 3, "speaker": "olu", "method": "user_confirmed", "confidence": "high"},
                {"sentence_id": 4, "speaker": "mina", "method": "contextual", "confidence": "medium"}
            ]})
            .to_string(),
        )
        .unwrap();
        let uri = "/app/transcripts/api/read/20260810?transcripts=1";

        // No confirmed voiceprint yet: a voice match alone names no owner.
        let markdown = read_markdown(root.path(), uri).await;
        assert!(markdown.contains("Speaker 1: Heard as mine"), "{markdown}");
        assert!(markdown.contains("Mina: Heard as Mina"), "{markdown}");
        assert!(
            markdown.contains("Olu (owner): Answered as mine"),
            "{markdown}"
        );
        assert!(
            markdown.contains("Speaker 2: Guessed as Mina"),
            "{markdown}"
        );

        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root.path(),
            "olu",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: vec![1.0, 0.0],
                cluster_size: 1,
                timestamp: "2026-08-01T00:00:00Z".to_owned(),
                evidence_tier: "test".to_owned(),
            },
        )
        .unwrap();
        let markdown = read_markdown(root.path(), uri).await;
        assert!(
            markdown.contains("Olu (owner): Heard as mine"),
            "{markdown}"
        );
        assert!(
            markdown.contains("Speaker 2: Guessed as Mina"),
            "{markdown}"
        );
    }
}
