// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Body app's routes against journals built from body windows and imports.

use std::fs;
use std::path::{Path, PathBuf};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};
use tower::ServiceExt;

use crate::api_router;
use crate::rows::window_read_probe;

pub(crate) const DAY: &str = "20260110";

pub(crate) async fn get(root: &Path, path: &str) -> (StatusCode, Value) {
    let response = api_router(root)
        .oneshot(Request::get(path).body(Body::empty()).expect("request builds"))
        .await
        .expect("router responds");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    (status, serde_json::from_slice(&body).expect("response JSON parses"))
}

fn zone(root: &Path, source: &str) {
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join(format!("imports/.{source}-zone.json")),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();
}

fn base_row(kind: RowKind, id: &str, source: &str, record_type: &str, label: &str) -> BodyRow {
    let mut row = BodyRow::new(kind, id.to_owned(), source, record_type, label.to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    row
}

pub(crate) fn reading(id: &str, at: &str) -> BodyRow {
    let mut row = base_row(
        RowKind::Reading,
        id,
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate",
    );
    row.at = Some(at.to_owned());
    row.value = Some("61".to_owned());
    row.unit = Some("count/min".to_owned());
    row
}

pub(crate) fn steps(id: &str, start: &str, end: &str, value: &str) -> BodyRow {
    let mut row = base_row(
        RowKind::Count,
        id,
        "apple_health",
        "HKQuantityTypeIdentifierStepCount",
        "step count",
    );
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.value = Some(value.to_owned());
    row.unit = Some("count".to_owned());
    row
}

pub(crate) fn workout(id: &str, start: &str, end: &str) -> BodyRow {
    let mut row = base_row(
        RowKind::Experience,
        id,
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "running",
    );
    row.experience = Some("workout".to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.values.insert("distance".to_owned(), "8123.4 m".to_owned());
    row
}

pub(crate) fn logged(id: &str, at: &str) -> BodyRow {
    let mut row = base_row(
        RowKind::Logged,
        id,
        "apple_health",
        "HKCategoryTypeIdentifierHeadache",
        "headache",
    );
    row.start = Some(at.to_owned());
    row.end = Some(at.to_owned());
    row.fields
        .insert("metadata".to_owned(), json!({"HKWasUserEntered": "1"}));
    row
}

pub(crate) fn dated(id: &str, day: &str) -> BodyRow {
    let mut row = base_row(RowKind::Dated, id, "oura", "daily_readiness", "readiness");
    row.source = "oura".to_owned();
    row.date = Some(format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..]));
    row.by = None;
    row
}

pub(crate) fn window(root: &Path, day: &str, stream: &str, key: &str, rows: &[BodyRow]) -> PathBuf {
    let dir = root.join("chronicle").join(day).join(stream).join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("body.jsonl"), encode_rows(rows)).unwrap();
    dir
}

fn strava_tile(root: &Path, day: &str, key: &str) {
    let dir = root.join("chronicle").join(day).join("import.strava").join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("workout.json"),
        json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 9001,
            "tile": {"index": 0, "count": 1},
            "workout": {"name": "Morning Run", "type": "Run", "start": "2026-01-10T07:00:00-07:00",
                "elapsed_seconds": 1800, "distance_m": 5000.0}
        })
        .to_string(),
    )
    .unwrap();
}

fn old_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/body_old_bundles").join(name)
}

const NATIVE_ID: &str = "body-01M70HDTD9N4P15X06CRDG2DWZ";

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn with_native(root: &Path) {
    copy_tree(&old_fixture("native").join(NATIVE_ID), &root.join("imports").join(NATIVE_ID));
}

fn with_python(root: &Path) {
    copy_tree(
        &old_fixture("python_era/20260304_080000"),
        &root.join("imports/20260304_080000"),
    );
}

fn new_import_with(root: &Path, id: &str, family: &str, attempt: Option<&str>) -> PathBuf {
    let dir = root.join("imports").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("body-touched.jsonl"), "").unwrap();
    fs::write(dir.join("body-progress.json"), r#"{"phase":"write"}"#).unwrap();
    fs::write(
        dir.join("manifest.json"),
        json!({"source_type": family}).to_string(),
    )
    .unwrap();
    if let Some(state) = attempt {
        fs::write(
            dir.join("import.json"),
            json!({"attempt": {"attempt_id":"a","generation":1,"state":state,"started_at_ms":1}})
                .to_string(),
        )
        .unwrap();
    }
    dir
}

#[tokio::test]
async fn a_new_journal_reads_as_empty_on_every_route() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    assert_eq!(
        get(root, "/app/body/api/index").await,
        (StatusCode::OK, json!({"coverage": null, "months": {}}))
    );
    assert_eq!(
        get(root, "/app/body/api/stats/202608").await,
        (StatusCode::OK, json!({}))
    );
    assert_eq!(
        get(root, "/app/body/api/stats/nope").await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, overview) = get(root, "/app/body/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["coverage"], Value::Null);
    assert_eq!(overview["imports"]["old"], json!([]));
    let (_, day) = get(root, "/app/body/api/day/20240101").await;
    assert_eq!(day["has_data"], false);
    let (status, _) = get(root, "/app/body/api/day/notaday").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, recent) = get(root, "/app/body/api/recent?before=20260801").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent, json!({"days": [], "has_more": false}));
}

#[tokio::test]
async fn the_routes_with_no_trends_serve_404_and_day_validation_holds() {
    let temporary = tempfile::tempdir().unwrap();
    let response = api_router(temporary.path())
        .oneshot(Request::get("/app/body/api/trends").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_overview_holds_each_source_its_days_and_every_import() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    window(root, "20260110", "import.apple_health", "090000_3600", &[reading("r1", "2026-01-10 09:05:00 -0700")]);
    window(root, "20260112", "import.apple_health", "090000_3600", &[reading("r2", "2026-01-12 09:05:00 -0700")]);
    window(root, "20260111", "import.oura", "000000_86400", &[dated("d1", "20260111")]);
    strava_tile(root, "20260111", "070000_300");

    let (status, overview) = get(root, "/app/body/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["coverage"], json!({"start": "20260110", "end": "20260112"}));
    let sources = overview["sources"].as_array().unwrap();
    assert_eq!(
        sources.iter().map(|item| item["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Apple Health", "Oura", "Strava"]
    );
    assert_eq!(sources[0]["first_day"], "20260110");
    assert_eq!(sources[0]["last_day"], "20260112");
    assert_eq!(sources[0]["days"], 2);
    assert_eq!(sources[0]["days_label"], "2 days");
    assert_eq!(sources[1]["days"], 1);
    assert_eq!(overview["days"], 3);
    assert!(overview["day_grid"].as_array().is_some_and(|grid| !grid.is_empty()));

    let (_, index) = get(root, "/app/body/api/index").await;
    assert_eq!(index["coverage"], overview["coverage"]);
    assert_eq!(index["months"], json!({"202601": 3}));
    let (_, stats) = get(root, "/app/body/api/stats/202601").await;
    assert_eq!(stats, json!({"20260110": 1, "20260111": 2, "20260112": 1}));
}

#[tokio::test]
async fn the_overview_index_and_stats_read_no_window() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    window(root, DAY, "import.apple_health", "090000_3600", &[reading("r1", "2026-01-10 09:05:00 -0700")]);
    window(root, "20260111", "import.oura", "000000_86400", &[dated("d1", "20260111")]);
    // Garbage in the window files: an overview that read them would still fail only by
    // returning less, so the count of reads is the instrument.
    fs::write(root.join("chronicle/20260110/import.apple_health/090000_3600/body.jsonl"), [0xff_u8]).unwrap();

    window_read_probe::reset();
    for path in [
        "/app/body/api/status",
        "/app/body/api/index",
        "/app/body/api/stats/202601",
        "/app/body/api/recent?before=20260201",
    ] {
        assert_eq!(get(root, path).await.0, StatusCode::OK, "{path}");
    }
    assert_eq!(window_read_probe::reads(), 0, "these routes opened a window");

    get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert!(window_read_probe::reads() > 0, "the probe counts a day's reads");
}

#[tokio::test]
async fn a_deleted_window_isnt_held_and_a_day_is_gone_when_all_are_deleted() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let live = window(root, DAY, "import.apple_health", "090000_3600", &[reading("r1", "2026-01-10 09:05:00 -0700")]);
    fs::write(live.join("tombstone.json"), "{}").unwrap();
    assert_eq!(get(root, "/app/body/api/index").await.1["coverage"], Value::Null);
}

#[tokio::test]
async fn a_day_lists_experiences_readings_counts_logged_and_documents_and_computes_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    zone(root, "apple_health");
    window(
        root,
        DAY,
        "import.apple_health",
        "060000_3600",
        &[
            workout("w1", "2026-01-10 06:31:00 -0700", "2026-01-10 07:20:00 -0700"),
            reading("r1", "2026-01-10 06:35:00 -0700"),
            steps("s1", "2026-01-10 06:40:00 -0700", "2026-01-10 06:45:00 -0700", "1200"),
            logged("l1", "2026-01-10 06:50:00 -0700"),
        ],
    );
    window(
        root,
        DAY,
        "import.apple_health",
        "210000_3600",
        &[
            reading("r2", "2026-01-10 21:10:00 -0700"),
            steps("s2", "2026-01-10 21:15:00 -0700", "2026-01-10 21:20:00 -0700", "2000.5"),
        ],
    );
    window(root, DAY, "import.oura", "000000_86400", &[dated("d1", DAY)]);
    strava_tile(root, DAY, "070000_300");

    let (status, day) = get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(day["has_data"], true);
    assert_eq!(day["day_label"], "January 10, 2026");
    let apple = &day["sources"].as_array().unwrap()[0];
    assert_eq!(apple["name"], "Apple Health");
    let experience = &apple["experiences"][0];
    assert_eq!(experience["kind"], "workout");
    assert_eq!(experience["start"], "6:31 AM");
    assert_eq!(experience["end"], "7:20 AM");
    assert_eq!(experience["by"], "Synthetic Watch");
    assert_eq!(experience["distance"], "8.1 km");
    let reading_item = &apple["readings"][0];
    assert_eq!(reading_item["name"], "heart rate");
    assert_eq!(reading_item["count"], 2);
    assert_eq!(reading_item["first"], "6:35 AM");
    assert_eq!(reading_item["last"], "9:10 PM");
    let count_item = &apple["counts"][0];
    assert_eq!(count_item["name"], "step count");
    assert_eq!(count_item["total"], "3,200.5");
    assert_eq!(apple["logged"][0]["name"], "headache");
    assert_eq!(apple["logged"][0]["who"], "you, in Synthetic Watch");
    assert_eq!(day["sources"][1]["documents"], json!([{"name": "readiness", "value": null}]));
    assert_eq!(day["strava"][0]["strava_activity_id"], "9001");

    let text = day.to_string();
    for retired in ["min", "max", "average", "avg", "comparison", "glucose", "range_label", "trends", "summary"] {
        assert!(!text.contains(&format!("\"{retired}\"")), "{retired}");
    }
    assert!(!text.contains("distance_unit"), "{text}");
    assert_eq!(day["earlier_imports"], json!([]));
}

#[tokio::test]
async fn a_row_two_windows_share_is_read_once_and_a_moved_row_isnt_counted() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    window(root, DAY, "import.apple_health", "090000_3600", &[reading("r1", "2026-01-10 09:05:00 -0700")]);
    window(root, DAY, "import.apple_health", "090000_3601", &[reading("r1", "2026-01-10 09:05:00 -0700")]);
    let mut moved = reading("r2", "2026-01-10 09:30:00 -0700");
    moved.kind = RowKind::Moved;
    window(root, DAY, "import.apple_health", "100000_3600", &[moved]);
    let (_, day) = get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert_eq!(day["sources"][0]["readings"][0]["count"], 1);
    assert_eq!(day["entry_total"], 1);
}

#[tokio::test]
async fn an_unreadable_window_is_said_and_the_rest_of_the_day_still_reads() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let bad = window(root, DAY, "import.apple_health", "090000_3600", &[]);
    fs::write(bad.join("body.jsonl"), "not json\n").unwrap();
    window(root, DAY, "import.apple_health", "100000_3600", &[reading("r1", "2026-01-10 10:05:00 -0700")]);
    let (_, day) = get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert_eq!(day["unreadable_windows"], 1);
    assert_eq!(day["sources"][0]["readings"][0]["count"], 1);
}

#[tokio::test]
async fn an_empty_day_keeps_its_nearest_days() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    window(root, "20260109", "import.apple_health", "090000_3600", &[reading("r1", "2026-01-09 09:05:00 -0700")]);
    window(root, "20260112", "import.apple_health", "090000_3600", &[reading("r2", "2026-01-12 09:05:00 -0700")]);
    let (_, day) = get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert_eq!(day["has_data"], false);
    assert_eq!(day["nearest"]["prev"], "20260109");
    assert_eq!(day["nearest"]["next"], "20260112");
}

#[tokio::test]
async fn a_new_path_import_alone_shows_no_old_import_line() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let dir = new_import_with(root, "20260110_120000", "apple_health", Some("completed"));
    fs::write(dir.join("summary.md"), "# Apple Health import\n").unwrap();
    let (_, overview) = get(root, "/app/body/api/status").await;
    assert_eq!(overview["imports"]["old"], json!([]));
    assert_eq!(overview["imports"]["new"].as_array().unwrap().len(), 1);
    let (_, day) = get(root, &format!("/app/body/api/day/{DAY}")).await;
    assert_eq!(day["earlier_imports"], json!([]));
}

#[tokio::test]
async fn a_finished_import_shows_its_summary_and_running_or_stopped_shows_how_far() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let finished = new_import_with(root, "20260110_120000", "apple_health", Some("completed"));
    fs::write(finished.join("summary.md"), "# Apple Health import\n\nwhat happened\n").unwrap();

    let stopped = new_import_with(root, "20260111_120000", "oura", Some("unconfirmed"));
    fs::write(
        stopped.join("body-progress.json"),
        r#"{"phase":"write","days_done":3,"days_total":120}"#,
    )
    .unwrap();

    let early = new_import_with(root, "20260112_120000", "apple_health", Some("unconfirmed"));
    fs::write(early.join("body-progress.json"), r#"{"phase":"scan","rows_scanned":1500}"#).unwrap();

    let running = new_import_with(root, "20260113_120000", "oura", None);
    fs::remove_file(running.join("manifest.json")).unwrap();
    fs::write(
        running.join("body-progress.json"),
        r#"{"phase":"write","days_done":5,"days_total":40}"#,
    )
    .unwrap();
    solstone_core_import::admit_running_attempt(root, "20260113_120000", 1, Some("oura")).unwrap();

    let (_, overview) = get(root, "/app/body/api/status").await;
    let imports = overview["imports"]["new"].as_array().unwrap();
    assert_eq!(imports.len(), 4);
    let by_id = |id: &str| imports.iter().find(|item| item["id"] == id).unwrap();

    let done = by_id("20260110_120000");
    assert_eq!(done["state"], "finished");
    assert!(done["summary"].as_str().unwrap().contains("what happened"));
    assert_eq!(done["progress"], Value::Null);
    assert_eq!(done["when"], "Jan 10, 2026, 12:00 PM");

    let halted = by_id("20260111_120000");
    assert_eq!(halted["state"], "stopped");
    assert_eq!(halted["summary"], Value::Null);
    assert_eq!(halted["progress"], "stopped partway: 3 of 120 days done.");

    let scanning = by_id("20260112_120000");
    assert_eq!(scanning["progress"], "stopped partway: 1,500 entries read so far.");

    let going = by_id("20260113_120000");
    assert_eq!(going["state"], "running");
    assert_eq!(going["source"], "Oura");
    assert_eq!(going["progress"], "still running: 5 of 40 days done.");
}

#[tokio::test]
async fn old_bundles_are_listed_by_source_and_span_and_say_how_to_import_again() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    with_native(root);
    with_python(root);
    let dexcom = root.join("imports/20240101_090000");
    fs::create_dir_all(dexcom.join("normalized")).unwrap();
    fs::write(dexcom.join("manifest.json"), r#"{"source_type":"dexcom_clarity"}"#).unwrap();
    fs::write(dexcom.join("normalized/2024-01.jsonl"), "").unwrap();

    let (_, overview) = get(root, "/app/body/api/status").await;
    let old = overview["imports"]["old"].as_array().unwrap();
    assert_eq!(old.len(), 3);
    let by_source = |source: &str, span_by: &str| {
        old.iter()
            .find(|item| item["source"] == source && item["span_by"] == span_by)
            .unwrap()
    };
    let native = by_source("Apple Health", "days");
    assert_eq!(native["span"], "Jan 1, 2026 to Jan 2, 2026, 2 days");
    assert!(native["line"].as_str().unwrap().contains("import again from a new Apple Health export"));
    let python = by_source("Apple Health", "months");
    assert_eq!(python["span"], "March 2026 (by month)");
    let dexcom = by_source("Dexcom Clarity", "months");
    let line = dexcom["line"].as_str().unwrap();
    assert!(line.contains("no importer"), "{line}");
    assert_eq!(dexcom["importer"], false);
}

#[tokio::test]
async fn a_day_page_says_an_old_import_holds_the_day_until_the_day_is_touched() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    with_native(root);
    let (_, day) = get(root, "/app/body/api/day/20260101").await;
    assert_eq!(day["has_data"], false);
    let earlier = day["earlier_imports"].as_array().unwrap();
    assert_eq!(earlier.len(), 1);
    assert_eq!(earlier[0]["scope"], "day");
    assert!(earlier[0]["line"].as_str().unwrap().contains("this day's body data"));
    window(root, "20260101", "import.apple_health", "000000_3600", &[reading("r1", "2026-01-01 00:05:00 -0700")]);
    let (_, day) = get(root, "/app/body/api/day/20260101").await;
    assert_eq!(day["earlier_imports"], json!([]));
}

#[tokio::test]
async fn a_python_era_line_says_the_month_and_never_says_the_day_is_covered() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    with_python(root);
    let (_, day) = get(root, "/app/body/api/day/20260315").await;
    let earlier = day["earlier_imports"].as_array().unwrap();
    assert_eq!(earlier.len(), 1, "{day}");
    assert_eq!(earlier[0]["scope"], "month");
    let line = earlier[0]["line"].as_str().unwrap();
    assert!(line.contains("March 2026"), "{line}");
    assert!(!line.contains("this day"), "{line}");
}

#[tokio::test]
async fn a_window_counts_what_it_holds_on_the_placed_clock() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    zone(root, "apple_health");
    window(root, DAY, "import.apple_health", "090000_3600", &[reading("r1", "2026-01-10 09:05:00 -0700"), reading("r2", "2026-01-10 09:45:00 -0700")]);
    window(root, DAY, "import.apple_health", "220000_3600", &[reading("r3", "2026-01-10 22:05:00 -0700")]);
    let (status, payload) = get(
        root,
        "/app/body/api/window?from=2026-01-10T09:00:00-07:00&to=2026-01-10T10:00:00-07:00",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["has_data"], true);
    assert_eq!(payload["entry_total"], 2);
    assert_eq!(payload["signals"][0]["label"], "heart rate");
    assert_eq!(payload["signals"][0]["count_label"], "2 readings");
    assert_eq!(payload["rows"].as_array().unwrap().len(), 2);
    assert_eq!(payload["rows"][0]["value"], "61");
    let text = payload.to_string();
    for retired in ["\"min\"", "\"max\"", "range_label\":\"6", "delta_label", "heart_rate", "glucose"] {
        assert!(!text.contains(retired), "{retired}");
    }
    let hourly = payload["hourly"].as_array().unwrap();
    assert_eq!(hourly[0]["label"], "9:00 AM");
    assert_eq!(hourly[0]["entry_total"], 2);
}

#[tokio::test]
async fn a_window_reads_strava_tiles_as_events() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    zone(root, "strava");
    strava_tile(root, DAY, "070000_300");
    let (_, payload) = get(
        root,
        "/app/body/api/window?from=2026-01-10T07:00:00-07:00&to=2026-01-10T08:00:00-07:00",
    )
    .await;
    assert_eq!(payload["has_data"], true);
    assert_eq!(payload["events"][0]["label"], "run");
    assert_eq!(payload["events"][0]["source"], "Strava");
    assert_eq!(payload["sources"]["names"], json!(["Strava"]));
}

#[tokio::test]
async fn a_window_refuses_bad_bounds_as_it_always_has() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    for path in [
        "/app/body/api/window",
        "/app/body/api/window?from=x&to=2026-01-10T00:00:00Z",
        "/app/body/api/window?from=2026-01-10T01:00:00Z&to=2026-01-10T00:00:00Z",
        "/app/body/api/window?from=2026-01-01T00:00:00Z&to=2026-01-10T00:00:00Z",
    ] {
        assert_eq!(get(root, path).await.0, StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(
        get(
            root,
            "/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00&to=2026-08-02T00%3A00%3A00%2B00%3A00"
        )
        .await
        .1["has_data"],
        false
    );
}

#[tokio::test]
async fn recent_lists_days_with_sources_and_pages_from_the_cursor() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    window(root, "20260105", "import.apple_health", "090000_3600", &[reading("a", "2026-01-05 09:05:00 -0700")]);
    window(root, "20260106", "import.oura", "000000_86400", &[dated("b", "20260106")]);
    let (status, recent) = get(root, "/app/body/api/recent?before=20260110&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent["days"][0]["day"], "20260106");
    assert_eq!(recent["days"][0]["sources"], json!(["Oura"]));
    assert_eq!(recent["days"][0]["windows"], 1);
    assert_eq!(recent["has_more"], true);
    assert_eq!(
        get(root, "/app/body/api/recent?before=nope").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(root, "/app/body/api/recent?before=20260110&limit=0").await.0,
        StatusCode::BAD_REQUEST
    );
}
