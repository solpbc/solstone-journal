// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Black-box contracts for native `solstone call health` journal-data routes.

use std::fs;
use std::io::Write;
use std::path::Path;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::{Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_convey_shell::router;
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, create_oplog_at},
};
use tower::ServiceExt;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("fixture");
        for path in ["facets", "entities", "chronicle", "talents"] {
            fs::create_dir_all(root.path().join(path)).expect("journal directory");
        }
        Self { root }
    }

    fn established(&self) {
        write_json(
            &self.root.path().join("config/journal.json"),
            json!({"setup":{"completed_at":1},"identity":{"name":"Health Owner","timezone":"UTC"},"providers":{"active":{"provider":"test"}}}),
        );
    }

    fn facet(&self, name: &str) {
        write_json(
            &self
                .root
                .path()
                .join("facets")
                .join(name)
                .join("facet.json"),
            json!({"name":name}),
        );
    }

    fn activities(&self, facet: &str, day: &str, rows: &[Value]) {
        write_jsonl(
            &self
                .root
                .path()
                .join("facets")
                .join(facet)
                .join("activities")
                .join(format!("{day}.jsonl")),
            rows,
        );
    }

    fn talent_guards(&self, now: chrono::DateTime<Utc>) {
        for day in [now.date_naive(), now.date_naive() - Duration::days(1)] {
            write_jsonl(
                &self
                    .root
                    .path()
                    .join("talents")
                    .join(format!("{}.jsonl", day.format("%Y%m%d"))),
                &[json!({"ts":now.timestamp_millis(),"status":"ok"})],
            );
        }
    }

    fn health_log(&self, day: &str, run: &str, rows: &[Value]) {
        let day = NaiveDate::parse_from_str(day, "%Y%m%d").expect("valid test day");
        let opened = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).expect("valid noon"))
            .single()
            .expect("unique UTC test instant");
        let mut writer = create_oplog_at(
            JournalRoot::open(self.root.path()).expect("fixture journal root"),
            "think",
            run,
            OplogFormat::Jsonl,
            opened,
        )
        .expect("canonical think oplog");
        for row in rows {
            writeln!(
                writer,
                "{}",
                serde_json::to_string(row).expect("fixture JSON row")
            )
            .expect("write fixture oplog row");
        }
        writer.flush().expect("flush fixture oplog");
    }

    fn screen_segment(&self, day: &str, segment: &str) {
        let directory = self.root.path().join("chronicle").join(day).join(segment);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
    }
}

fn write_json(path: &Path, value: Value) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string(&value).expect("json")),
    )
    .expect("write");
}

fn write_jsonl(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    fs::write(
        path,
        format!(
            "{}\n",
            rows.iter()
                .map(|row| serde_json::to_string(row).expect("json"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .expect("jsonl");
}

async fn get(path: &str, fixture: &Fixture) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = router(fixture.root.path().to_path_buf())
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, headers, body)
}

fn body_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("JSON response")
}

fn normalize_report(value: &mut Value) {
    value
        .as_object_mut()
        .expect("report")
        .remove("generated_at");
    for note in value["notes"].as_array_mut().expect("notes") {
        note.as_object_mut().expect("note").remove("detected_at");
    }
}

#[tokio::test]
async fn summary_route_inherits_unestablished_session_gate() {
    let fixture = Fixture::new();
    let (status, headers, _) = get("/api/health/summary", &fixture).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers.get(header::LOCATION).expect("location"), "/init");
}

#[tokio::test]
async fn summary_returns_complete_wire_shape() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/summary?day=20260401", &fixture).await;
    assert_eq!(status, StatusCode::OK);
    let body = body_json(&bytes);
    for field in [
        "generated_at",
        "range",
        "facets",
        "capture_health",
        "synthesis_health",
        "consumer_signal",
        "segment_backlog",
        "notes",
        "brain_health",
    ] {
        assert!(body.get(field).is_some(), "missing {field}");
    }
    assert!(
        body["consumer_signal"]
            .get("profile_entities_total")
            .is_some()
    );
}

#[tokio::test]
async fn summary_and_full_share_the_same_report_builder() {
    let fixture = Fixture::new();
    fixture.established();
    let (_, _, summary) = get("/api/health/summary?day=20260401", &fixture).await;
    let (_, _, full) = get("/api/health/full?day=20260401", &fixture).await;
    let mut summary = body_json(&summary);
    let mut full = body_json(&full);
    summary
        .as_object_mut()
        .expect("summary")
        .remove("generated_at");
    full.as_object_mut().expect("full").remove("generated_at");
    for body in [&mut summary, &mut full] {
        for note in body["notes"].as_array_mut().expect("notes") {
            note.as_object_mut().expect("note").remove("detected_at");
        }
    }
    assert_eq!(summary, full);
}

#[tokio::test]
async fn optional_report_fields_survive_as_json_null() {
    let fixture = Fixture::new();
    fixture.established();
    let (_, _, bytes) = get("/api/health/summary?day=20260401", &fixture).await;
    let body = body_json(&bytes);
    assert_eq!(body["capture_health"]["coverage_ratio"], Value::Null);
    assert_eq!(body["capture_health"]["last_segment_at"], Value::Null);
}

#[tokio::test]
async fn summary_rejects_malformed_day() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/summary?day=nope", &fixture).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&bytes)["reason_code"], "invalid_request_value");
}

#[tokio::test]
async fn range_accepts_explicit_window() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get(
        "/api/health/range?day_from=20260401&day_to=20260402",
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(&bytes)["range"], json!(["20260401", "20260402"]));
}

#[tokio::test]
async fn range_defaults_when_both_endpoints_are_absent() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/range", &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body_json(&bytes)["range"].as_array().expect("range").len(),
        2
    );
}

#[tokio::test]
async fn range_rejects_one_sided_window() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/range?day_from=20260401", &fixture).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&bytes)["detail"], "both endpoints or neither");
}

#[tokio::test]
async fn range_rejects_inverted_window() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get(
        "/api/health/range?day_from=20260402&day_to=20260401",
        &fixture,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&bytes)["detail"], "day_from must be <= day_to");
}

#[tokio::test]
async fn pipeline_is_compact_and_preserves_root_order() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, headers, bytes) = get("/api/health/pipeline?day=20260401", &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).expect("content type"),
        "application/json"
    );
    let text = String::from_utf8(bytes).expect("UTF-8 JSON");
    assert!(!text.contains("\n") && !text.contains(": "));
    let keys = [
        "day",
        "generated_at",
        "status",
        "anomalies",
        "runs",
        "talents",
        "activities",
        "exhausted_segments",
    ];
    let positions = keys.map(|key| text.find(&format!("\"{key}\"")).expect("ordered key"));
    assert!(positions.windows(2).all(|window| window[0] < window[1]));
}

#[tokio::test]
async fn pipeline_requires_day() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/pipeline", &fixture).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&bytes)["reason_code"], "missing_required_field");
    assert_eq!(body_json(&bytes)["detail"], "day is required");
}

#[tokio::test]
async fn pipeline_rejects_non_calendar_day() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/health/pipeline?day=20260230", &fixture).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(&bytes)["reason_code"], "invalid_request_value");
    assert_eq!(
        body_json(&bytes)["detail"],
        "day must be a calendar date in YYYYMMDD format"
    );
}

fn prepared_report_fixture() -> (Fixture, chrono::DateTime<Utc>, String) {
    let fixture = Fixture::new();
    fixture.established();
    let now = Utc::now();
    let day = now.format("%Y%m%d").to_string();
    fixture.talent_guards(now);
    fixture.facet("work");
    fixture.facet("personal");
    fixture.activities(
        "work",
        &day,
        &[json!({
            "id":"work-rich",
            "created_at":(now - Duration::days(20)).timestamp_millis(),
            "segments":["000000_3600"],
            "participation":true,
            "story":{"summary":"done"},
            "edits":[{"actor":"owner"}],
            "source":"anticipated",
            "start":"2020-01-01T00:00:00Z",
        })],
    );
    fixture.activities(
        "personal",
        &day,
        &[json!({"id":"personal-rich","segments":["010000_3600"]})],
    );
    write_json(
        &fixture.root.path().join("entities/a/entity.json"),
        json!({"id":"a","name":"A"}),
    );
    write_json(
        &fixture.root.path().join("entities/b/entity.json"),
        json!({"id":"b","name":"B"}),
    );
    (fixture, now, day)
}

#[tokio::test]
async fn rich_fixture_matches_the_complete_report_contract() {
    let (fixture, now, day) = prepared_report_fixture();
    let (status, _, bytes) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    assert_eq!(status, StatusCode::OK);
    let mut body = body_json(&bytes);
    normalize_report(&mut body);
    let last_segment = now
        .date_naive()
        .and_hms_opt(2, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(
        body,
        json!({
            "range":[day.clone(),day],
            "facets":["personal","work"],
            "capture_health":{
                "hours_with_capture":2,"hours_total":24,"coverage_ratio":null,
                "facets_with_recent_capture":["personal","work"],"facets_silent_24h":[],
                "last_segment_at":last_segment
            },
            "synthesis_health":{
                "activities_count":2,"activities_with_participation":1,"activities_with_story":1,
                "activities_user_edited":1,"activities_anticipated_unfilled":1,
                "talent_run_failures_24h":0,"talent_degraded_outputs_24h":0,
                "indexer_last_rebuild_at":null
            },
            "consumer_signal":{"profile_entities_total":2},
            "segment_backlog":{
                "not_thought":0,"days_with_backlog":0,"errors":[],"not_sensed":0,
                "awaiting_analysis_text":null,"last_drained_at":null,"drain_state":"realtime",
                "display_powersave_detectable":false
            },
            "notes":[
                {"severity":"warn","category":"synthesis","message":"indexer database missing at journal/indexer/journal.sqlite; search-backed consumers may be stale.","detail_pointer":null},
                {"severity":"info","category":"capture","message":"coverage_ratio unavailable in v1 — expected-hours denominator arrives Sprint 5+","detail_pointer":"solstone/think/surfaces/health.py"},
                {"severity":"info","category":"synthesis","message":"corrections roll-up not available — corrections support arrives Sprint 5+","detail_pointer":"solstone/think/surfaces/health.py"}
            ],
            "brain_health":{
                "snapshot":{
                    "state":"unknown","headline":"thinking status unavailable","reason_code":"configuration_invalid","reason_text":"configuration invalid","failing_component":null,
                    "action":{"label":"open thinking","href":"/app/thinking/#main"},
                    "identity":{"lane":null,"provider":"test","model":""},
                    "evidence":{"observed_at":null,"age_seconds":null,"age_text":null},
                    "components":{
                        "generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null},
                        "cogitate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}
                    },
                    "progressing":false
                },
                "lines":["Brain Health","  thinking status unavailable","  configuration invalid","  → open thinking: /app/thinking/#main"]
            }
        })
    );
}

#[tokio::test]
async fn activity_fixture_variation_changes_only_synthesis_fields() {
    let (fixture, _, day) = prepared_report_fixture();
    let (_, _, baseline) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    fixture.activities(
        "work",
        &day,
        &[
            json!({"id":"work-rich","created_at":1,"segments":["000000_3600"],"participation":true,"story":{"summary":"done"},"edits":[{"actor":"owner"}],"source":"anticipated","start":"2020-01-01T00:00:00Z"}),
            json!({"id":"activity-only"}),
        ],
    );
    let (_, _, changed) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    let mut baseline = body_json(&baseline);
    let mut changed = body_json(&changed);
    normalize_report(&mut baseline);
    normalize_report(&mut changed);
    assert_eq!(baseline["synthesis_health"]["activities_count"], 2);
    assert_eq!(changed["synthesis_health"]["activities_count"], 3);
    baseline.as_object_mut().unwrap().remove("synthesis_health");
    changed.as_object_mut().unwrap().remove("synthesis_health");
    assert_eq!(baseline, changed);
}

#[tokio::test]
async fn entity_fixture_variation_changes_only_consumer_signal_fields() {
    let fixture = Fixture::new();
    fixture.established();
    let now = Utc::now();
    let day = now.format("%Y%m%d").to_string();
    fixture.talent_guards(now);
    let (_, _, baseline) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    write_json(
        &fixture.root.path().join("entities/entity/entity.json"),
        json!({"id":"entity","name":"Entity"}),
    );
    let (_, _, changed) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    let mut baseline = body_json(&baseline);
    let mut changed = body_json(&changed);
    normalize_report(&mut baseline);
    normalize_report(&mut changed);
    assert_eq!(baseline["consumer_signal"]["profile_entities_total"], 0);
    assert_eq!(changed["consumer_signal"]["profile_entities_total"], 1);
    baseline.as_object_mut().unwrap().remove("consumer_signal");
    changed.as_object_mut().unwrap().remove("consumer_signal");
    assert_eq!(baseline, changed);
}

#[tokio::test]
async fn segment_fixture_variation_changes_only_backlog_fields() {
    let fixture = Fixture::new();
    fixture.established();
    let now = Utc::now();
    let day = now.format("%Y%m%d").to_string();
    fixture.talent_guards(now);
    let (_, _, baseline) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    fixture.screen_segment(&day, "120000_60");
    fixture.health_log(&day, "segment", &[json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"})]);
    let marker = fixture
        .root
        .path()
        .join("chronicle")
        .join(&day)
        .join("health/stream.updated");
    fs::write(marker, "updated\n").unwrap();
    let (_, _, changed) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    let mut baseline = body_json(&baseline);
    let mut changed = body_json(&changed);
    normalize_report(&mut baseline);
    normalize_report(&mut changed);
    assert_eq!(baseline["segment_backlog"]["not_thought"], 0);
    assert_eq!(changed["segment_backlog"]["not_thought"], 1);
    baseline.as_object_mut().unwrap().remove("segment_backlog");
    changed.as_object_mut().unwrap().remove("segment_backlog");
    assert_eq!(baseline, changed);
}

#[tokio::test]
async fn pipeline_route_returns_fixture_values_not_zero_defaults() {
    let fixture = Fixture::new();
    fixture.established();
    let day = Utc::now().format("%Y%m%d").to_string();
    fixture.health_log(
        &day,
        "daily",
        &[json!({"event":"run.complete","day":day,"mode":"daily","duration_ms":20})],
    );
    fixture.health_log(
        &day,
        "activity",
        &[
            json!({"event":"run.complete","day":day,"mode":"activity","duration_ms":30}),
            json!({"event":"talent.dispatch","day":day,"mode":"activity","name":"schedule"}),
            json!({"event":"activity.detected","day":day,"mode":"activity"}),
            json!({"event":"activity.persisted","day":day,"mode":"activity"}),
        ],
    );
    let (status, _, bytes) = get(&format!("/api/health/pipeline?day={day}"), &fixture).await;
    assert_eq!(status, StatusCode::OK);
    let body = body_json(&bytes);
    assert_eq!(body["status"], "healthy");
    assert_eq!(
        body["runs"]["daily"],
        json!({"count":1,"duration_ms_total":20})
    );
    assert_eq!(
        body["runs"]["activity"],
        json!({"count":1,"duration_ms_total":30})
    );
    assert_eq!(
        body["activities"],
        json!({"detected":1,"persisted":1,"talents_fired":true})
    );
    assert_eq!(body["talents"]["dispatched"], 1);
}

#[tokio::test]
async fn degraded_report_inputs_stay_200_but_internal_failures_are_safe_500s() {
    let fixture = Fixture::new();
    fixture.established();
    let now = Utc::now();
    let day = now.format("%Y%m%d").to_string();
    fixture.talent_guards(now);
    fs::write(fixture.root.path().join("talents/20260401.jsonl"), b"{").unwrap();
    let (status, _, bytes) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body_json(&bytes)["synthesis_health"]["talent_run_failures_24h"],
        Value::Null
    );
    assert!(
        body_json(&bytes)["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note["severity"] == "warn")
    );

    let past = (now.date_naive() - Duration::days(1))
        .format("%Y%m%d")
        .to_string();
    fixture.screen_segment(&past, "120000_60");
    let (status, _, bytes) = get(&format!("/api/health/pipeline?day={past}"), &fixture).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(&bytes)["status"], "unknown");

    fs::remove_dir_all(fixture.root.path().join("talents")).unwrap();
    fs::write(fixture.root.path().join("talents"), "not a directory").unwrap();
    let (status, _, bytes) = get(&format!("/api/health/summary?day={day}"), &fixture).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body_json(&bytes)["reason_code"], "health_report_failed");
    assert_eq!(body_json(&bytes)["detail"], "health report unavailable");
}
