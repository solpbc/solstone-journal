// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use solstone_core_system_health::{FilesystemSegmentSource, list_location_only_segments};

use crate::day::{invalid_day, valid_day};
use crate::{AppState, TranscriptError};

pub(crate) fn location_reading_count(dir: &Path) -> Option<u64> {
    let text = fs::read_to_string(dir.join("location.jsonl")).ok()?;
    let mut lines = text.lines();
    if let Some(first) = lines.next() {
        if first.contains("solstone.location.segment/1") {
            let count = lines
                .filter(|line| line.contains("solstone.location.fix/1"))
                .count() as u64;
            Some(count)
        } else {
            let count = text.lines().filter(|line| !line.is_empty()).count() as u64;
            Some(count)
        }
    } else {
        Some(0)
    }
}

pub(crate) async fn list_location_segments(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    if !valid_day(&day) {
        return invalid_day();
    }
    let now = state.clock.now();
    let journal_root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsLocation,
        move || {
            let rows = match list_location_only_segments(
                &FilesystemSegmentSource,
                &journal_root,
                &day,
                now,
            )
            .map_err(TranscriptError::health)
            {
                Ok(rows) => rows,
                Err(error) => return error.response(),
            };

            let mut segments = Vec::new();
            for row in rows {
                let physical_dir = crate::segment_media::physical_segment_dir(
                    &journal_root,
                    &day,
                    &row.stream,
                    &row.key,
                );
                let readings = location_reading_count(&physical_dir);
                let mut obj = serde_json::Map::new();
                obj.insert("key".to_owned(), Value::String(row.key));
                obj.insert("stream".to_owned(), Value::String(row.stream));
                obj.insert("start".to_owned(), Value::String(row.start));
                obj.insert("end".to_owned(), Value::String(row.end));
                if let Some(count) = readings {
                    obj.insert("location_readings".to_owned(), Value::Number(count.into()));
                }
                segments.push(Value::Object(obj));
            }

            Json(json!({ "segments": segments })).into_response()
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use chrono::Utc;
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;
    use crate::Clock;

    fn write_file(dir: &Path, name: &str, content: impl AsRef<[u8]>) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), content).unwrap();
    }

    fn test_app(root: PathBuf) -> axum::Router {
        crate::router(root, Clock::system(), || {
            axum::response::Response::new(Body::empty())
        })
    }

    async fn request_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    fn device_ingest_event_line(stream: &str, day: &str, segment: &str) -> String {
        serde_json::json!({
            "record_type": "device_ingest",
            "record_version": 1,
            "outcome": "accepted",
            "protocol_version": 3,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "",
            "stream": stream,
            "day": day,
            "segment": segment,
            "files": [],
            "meta": {},
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn location_reading_count_variants() {
        let temp = TempDir::new().unwrap();
        let p = temp.path();

        // 1. iPhone header + fixes + visit
        let d1 = p.join("iphone");
        fs::create_dir_all(&d1).unwrap();
        fs::write(
            d1.join("location.jsonl"),
            "{\"record_type\":\"solstone.location.segment/1\"}\n{\"record_type\":\"solstone.location.fix/1\"}\n{\"record_type\":\"solstone.location.visit/1\"}\n{\"record_type\":\"solstone.location.fix/1\"}\n",
        )
        .unwrap();
        assert_eq!(location_reading_count(&d1), Some(2));

        // 2. Android 3 non-empty lines
        let d2 = p.join("android");
        fs::create_dir_all(&d2).unwrap();
        fs::write(
            d2.join("location.jsonl"),
            "{\"lat\":1.0}\n{\"lat\":2.0}\n{\"lat\":3.0}\n",
        )
        .unwrap();
        assert_eq!(location_reading_count(&d2), Some(3));

        // 3. Header + visit only
        let d3 = p.join("visits_only");
        fs::create_dir_all(&d3).unwrap();
        fs::write(
            d3.join("location.jsonl"),
            "{\"record_type\":\"solstone.location.segment/1\"}\n{\"record_type\":\"solstone.location.visit/1\"}\n",
        )
        .unwrap();
        assert_eq!(location_reading_count(&d3), Some(0));

        // 4. Invalid UTF-8
        let d4 = p.join("invalid_utf8");
        fs::create_dir_all(&d4).unwrap();
        fs::write(d4.join("location.jsonl"), [0xff, 0xfe]).unwrap();
        assert_eq!(location_reading_count(&d4), None);
    }

    #[tokio::test]
    async fn get_location_segments_endpoint() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let day = "20260101";

        // Seg 1: phone / 090000_60, count 1
        let s1 = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("090000_60");
        write_file(&s1, "location.jsonl", "{\"lat\":1}\n");

        // Seg 2: watch / 090000_60, count 0 (header + visit only)
        let s2 = root
            .join("chronicle")
            .join(day)
            .join("watch")
            .join("090000_60");
        write_file(
            &s2,
            "location.jsonl",
            "{\"record_type\":\"solstone.location.segment/1\"}\n{\"record_type\":\"solstone.location.visit/1\"}\n",
        );

        // Seg 3: phone / 100000_60, producer-shaped
        let s3 = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("100000_60");
        write_file(&s3, "location.jsonl", "{\"lat\":1}\n{\"lat\":2}\n");
        write_file(&s3, "stream.json", "{}\n");
        write_file(
            &s3,
            "events.jsonl",
            device_ingest_event_line("phone", day, "100000_60"),
        );

        // Seg 4: mixed (has audio.jsonl) -> excluded
        let s4 = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("110000_60");
        write_file(&s4, "location.jsonl", "{\"lat\":1}\n");
        write_file(&s4, "audio.jsonl", "{\"row_type\":\"audio_transcript\"}\n");

        // Seg 5: bookkeeping without location.jsonl -> excluded
        let s5 = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("120000_60");
        write_file(&s5, "stream.json", "{}\n");
        write_file(
            &s5,
            "events.jsonl",
            device_ingest_event_line("phone", day, "120000_60"),
        );

        // Seg 6: _default named directory -> dropped
        let s6 = root
            .join("chronicle")
            .join(day)
            .join("_default")
            .join("130000_60");
        write_file(&s6, "location.jsonl", "{\"lat\":1}\n");

        let app = test_app(root.to_path_buf());

        // Valid day with rows
        let (status, res) =
            request_json(app.clone(), &format!("/app/transcripts/api/location/{day}")).await;
        assert_eq!(status, StatusCode::OK);
        let segments = res["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 3);

        // Ordered by start, then stream:
        // 09:00 phone, 09:00 watch, 10:00 phone
        assert_eq!(segments[0]["key"], "090000_60");
        assert_eq!(segments[0]["stream"], "phone");
        assert_eq!(segments[0]["location_readings"], 1);

        assert_eq!(segments[1]["key"], "090000_60");
        assert_eq!(segments[1]["stream"], "watch");
        assert_eq!(segments[1]["location_readings"], 0);

        assert_eq!(segments[2]["key"], "100000_60");
        assert_eq!(segments[2]["stream"], "phone");
        assert_eq!(segments[2]["location_readings"], 2);

        // Invalid day format: 404
        let (status, _) = request_json(app.clone(), "/app/transcripts/api/location/invalid").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Missing day: 200 with empty list
        let (status, res) =
            request_json(app.clone(), "/app/transcripts/api/location/20260202").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(res["segments"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn mixed_day_parity_with_and_without_location() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let day = "20260101";

        let seg_dir = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("100000_60");
        write_file(
            &seg_dir,
            "audio.jsonl",
            "{\"_solstone_processing\":{\"handler\":\"transcribe\"}}\n{\"row_type\":\"audio_transcript\"}\n",
        );
        write_file(&seg_dir, "location.jsonl", "{\"lat\":1}\n");

        let app = test_app(root.to_path_buf());

        // Snapshot day, segments, ranges, index, month-stats with location.jsonl
        let (s_day_with, b_day_with) =
            request_json(app.clone(), &format!("/app/transcripts/api/day/{day}")).await;
        let (s_seg_with, b_seg_with) =
            request_json(app.clone(), &format!("/app/transcripts/api/segments/{day}")).await;
        let (s_rng_with, b_rng_with) =
            request_json(app.clone(), &format!("/app/transcripts/api/ranges/{day}")).await;
        let (s_idx_with, b_idx_with) =
            request_json(app.clone(), "/app/transcripts/api/index").await;
        let (s_mon_with, b_mon_with) =
            request_json(app.clone(), "/app/transcripts/api/stats/202601").await;

        // Remove location.jsonl
        fs::remove_file(seg_dir.join("location.jsonl")).unwrap();

        let (s_day_without, b_day_without) =
            request_json(app.clone(), &format!("/app/transcripts/api/day/{day}")).await;
        let (s_seg_without, b_seg_without) =
            request_json(app.clone(), &format!("/app/transcripts/api/segments/{day}")).await;
        let (s_rng_without, b_rng_without) =
            request_json(app.clone(), &format!("/app/transcripts/api/ranges/{day}")).await;
        let (s_idx_without, b_idx_without) =
            request_json(app.clone(), "/app/transcripts/api/index").await;
        let (s_mon_without, b_mon_without) =
            request_json(app.clone(), "/app/transcripts/api/stats/202601").await;

        assert_eq!(s_day_with, s_day_without);
        assert_eq!(b_day_with, b_day_without);
        assert_eq!(s_seg_with, s_seg_without);
        assert_eq!(b_seg_with, b_seg_without);
        assert_eq!(s_rng_with, s_rng_without);
        assert_eq!(b_rng_with, b_rng_without);
        assert_eq!(s_idx_with, s_idx_without);
        assert_eq!(b_idx_with, b_idx_without);
        assert_eq!(s_mon_with, s_mon_without);
        assert_eq!(b_mon_with, b_mon_without);

        // Put location.jsonl back first so location file exists
        write_file(&seg_dir, "location.jsonl", "{\"lat\":1}\n");

        // Add a location-only day in that month
        let loc_day = "20260102";
        let loc_seg_dir = root
            .join("chronicle")
            .join(loc_day)
            .join("phone")
            .join("110000_60");
        write_file(&loc_seg_dir, "location.jsonl", "{\"lat\":1}\n");
        write_file(
            &root.join("chronicle").join(loc_day).join("phone"),
            "stream.json",
            "{\"device_id\":\"dev-1\"}\n",
        );
        write_file(
            &loc_seg_dir,
            "events.jsonl",
            "{\"event\":\"device_ingest\"}\n",
        );

        let system_dir = root.join("system_talents");
        let apps_dir = root.join("apps");
        fs::create_dir_all(&system_dir).unwrap();
        fs::create_dir_all(&apps_dir).unwrap();

        let segments = solstone_core_system_health::FilesystemSegmentSource;
        let health = solstone_core_system_health::FilesystemHealthLogSource::new(root);
        let writer = solstone_core_journal_stats_cli::FilesystemDayCacheWriter;

        // Set input mtimes slightly in the past before scan_day so the cache is strictly newer than inputs
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        let file_times = fs::FileTimes::new().set_modified(past);
        for entry in fs::read_dir(&seg_dir).unwrap() {
            let entry = entry.unwrap();
            let _ = fs::File::open(entry.path()).and_then(|f| f.set_times(file_times));
        }
        for entry in fs::read_dir(&loc_seg_dir).unwrap() {
            let entry = entry.unwrap();
            let _ = fs::File::open(entry.path()).and_then(|f| f.set_times(file_times));
        }

        // Before scan_day writes stats.json, loc_day is absent from the month object
        let (s_mon_no_cache, b_mon_no_cache) =
            request_json(app.clone(), "/app/transcripts/api/stats/202601").await;
        assert_eq!(s_mon_no_cache, StatusCode::OK);
        assert_eq!(b_mon_no_cache.get(loc_day), None);

        // Run scan_day for day
        solstone_core_journal_stats_cli::scan_day(
            solstone_core_journal_stats_cli::DayScanRequest {
                journal_root: root,
                day,
                now: Utc::now(),
                system_talent_root: &system_dir,
                apps_root: &apps_dir,
                talent_overrides: None,
                segment_source: &segments,
                health_source: &health,
                cache_writer: &writer,
            },
        )
        .unwrap();

        // Run scan_day for loc_day
        solstone_core_journal_stats_cli::scan_day(
            solstone_core_journal_stats_cli::DayScanRequest {
                journal_root: root,
                day: loc_day,
                now: Utc::now(),
                system_talent_root: &system_dir,
                apps_root: &apps_dir,
                talent_overrides: None,
                segment_source: &segments,
                health_source: &health,
                cache_writer: &writer,
            },
        )
        .unwrap();

        let (s_mon_with_cache, b_mon_with_cache) =
            request_json(app.clone(), "/app/transcripts/api/stats/202601").await;
        assert_eq!(s_mon_with_cache, StatusCode::OK);
        assert_eq!(b_mon_with_cache.get(loc_day), None);

        // Remove location.jsonl from day, re-set mtime on remaining files, re-scan day
        fs::remove_file(seg_dir.join("location.jsonl")).unwrap();
        for entry in fs::read_dir(&seg_dir).unwrap() {
            let entry = entry.unwrap();
            let _ = fs::File::open(entry.path()).and_then(|f| f.set_times(file_times));
        }
        solstone_core_journal_stats_cli::scan_day(
            solstone_core_journal_stats_cli::DayScanRequest {
                journal_root: root,
                day,
                now: Utc::now(),
                system_talent_root: &system_dir,
                apps_root: &apps_dir,
                talent_overrides: None,
                segment_source: &segments,
                health_source: &health,
                cache_writer: &writer,
            },
        )
        .unwrap();

        let (s_mon_without_cache, b_mon_without_cache) =
            request_json(app.clone(), "/app/transcripts/api/stats/202601").await;
        assert_eq!(s_mon_without_cache, StatusCode::OK);
        assert_eq!(b_mon_with_cache, b_mon_without_cache);
    }
}
