// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's hide for a day of body data, through the routes: the hide and
//! unhide, a hidden day on the day, overview and window routes, and the part of
//! a night or workout that started on a hidden day leaving the days after it.

use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_format::content::body_hidden_days::hidden_days_path;
use solstone_core_format::content::body_records::{BodyRow, RowKind};
use tower::ServiceExt;

use crate::api_router;
use crate::app_tests::{dated, get, reading, window};

const START: &str = "20261103";
const NEXT: &str = "20261104";

async fn post(root: &Path, path: &str, body: &str) -> (StatusCode, Value) {
    let response = api_router(root)
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    (
        status,
        serde_json::from_slice(&bytes).expect("response JSON parses"),
    )
}

async fn set_hidden(root: &Path, days: &[&str], hidden: bool) -> (StatusCode, Value) {
    post(
        root,
        "/app/body/api/hidden",
        &json!({"days": days, "hidden": hidden}).to_string(),
    )
    .await
}

fn zone(root: &Path, source: &str) {
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join(format!("imports/.{source}-zone.json")),
        json!({"zone": "America/Denver"}).to_string(),
    )
    .unwrap();
}

fn sleep_row(kind: RowKind, id: &str, of: Option<&str>, start: &str, end: &str) -> BodyRow {
    let mut row = BodyRow::new(
        kind,
        id.to_owned(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "sleep".to_owned(),
    );
    row.experience = Some("sleep".to_owned());
    row.of = of.map(str::to_owned);
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row
}

/// A night from 23:10 on the start day to 06:50 the next morning, with its
/// later piece and a stage after midnight on the next day; the next day's own
/// reading, and its own night from 22:30.
fn a_night_crossing_midnight(root: &Path) {
    zone(root, "apple_health");
    let night_start = "2026-11-03 23:10:00 -0700";
    let night_end = "2026-11-04 06:50:00 -0700";
    window(
        root,
        START,
        "import.apple_health",
        "230000_3600",
        &[sleep_row(
            RowKind::Experience,
            "n1",
            None,
            night_start,
            night_end,
        )],
    );
    let mut tile = sleep_row(
        RowKind::ExperienceTile,
        "n1:1",
        Some("n1"),
        night_start,
        night_end,
    );
    tile.index = Some(1);
    tile.count = Some(8);
    let mut stage = sleep_row(
        RowKind::Stage,
        "s1",
        Some("n1"),
        "2026-11-04 01:30:00 -0700",
        "2026-11-04 01:50:00 -0700",
    );
    stage.label = "deep".to_owned();
    window(
        root,
        NEXT,
        "import.apple_health",
        "010000_3600",
        &[tile, stage],
    );
    window(
        root,
        NEXT,
        "import.apple_health",
        "120000_3600",
        &[reading("r1", "2026-11-04 12:05:00 -0700")],
    );
    window(
        root,
        NEXT,
        "import.apple_health",
        "220000_3600",
        &[sleep_row(
            RowKind::Experience,
            "n2",
            None,
            "2026-11-04 22:30:00 -0700",
            "2026-11-05 06:10:00 -0700",
        )],
    );
}

fn experience_kinds(day: &Value) -> Vec<(String, bool)> {
    day["sources"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|source| source["experiences"].as_array().unwrap().clone())
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["continuing"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn reading_names(day: &Value) -> Vec<String> {
    day["sources"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|source| source["readings"].as_array().unwrap().clone())
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

async fn window_on(root: &Path, from: &str, to: &str) -> Value {
    let path = format!(
        "/app/body/api/window?from={}&to={}",
        from.replace('+', "%2B"),
        to.replace('+', "%2B")
    );
    let (status, payload) = get(root, &path).await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    payload
}

#[tokio::test]
async fn a_hide_then_an_unhide_and_a_repeat_answers_the_same() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    let (_, before) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(before["hidden"], false);
    assert_eq!(before["has_data"], true);

    let (status, saved) = set_hidden(root, &[START], true).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved, json!({"days": [START], "search_queued": true}));
    let (_, day) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(day["hidden"], true, "{day}");
    assert_eq!(day["has_data"], false);
    assert_eq!(day["nearest"]["next"], NEXT);
    for gone in ["sources", "strava", "earlier_imports", "entry_total"] {
        assert!(day.get(gone).is_none(), "{gone}: {day}");
    }

    // A repeat changes nothing and answers the same.
    assert_eq!(set_hidden(root, &[START], true).await, (status, saved));

    let (status, saved) = set_hidden(root, &[START], false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, json!({"days": [], "search_queued": true}));
    let (_, day) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(day, before);
}

#[tokio::test]
async fn a_bad_request_is_refused_and_changes_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    set_hidden(root, &[START], true).await;
    let store = fs::read(hidden_days_path(root)).unwrap();
    for body in [
        json!({"days": ["2026-11-04"], "hidden": true}),
        json!({"days": [NEXT, "20261332"], "hidden": true}),
        json!({"days": [NEXT, 20261105], "hidden": true}),
        json!({"days": [], "hidden": true}),
        json!({"days": [NEXT]}),
        json!({"days": NEXT, "hidden": true}),
        json!([NEXT]),
    ] {
        let (status, _) = post(root, "/app/body/api/hidden", &body.to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, _) = post(root, "/app/body/api/hidden", "{not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fs::read(hidden_days_path(root)).unwrap(), store);
}

#[tokio::test]
async fn a_damaged_store_refuses_a_hide_and_every_day_shows() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    let path = hidden_days_path(root);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"days\": [\"20261103\"").unwrap();

    let (status, refusal) = set_hidden(root, &[NEXT], true).await;
    assert!(!status.is_success(), "{refusal}");
    assert_eq!(fs::read(&path).unwrap(), b"{\"days\": [\"20261103\"");

    let (_, day) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(day["hidden"], false);
    assert_eq!(day["hidden_unreadable"], true);
    assert_eq!(day["has_data"], true);
    let (_, next) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert!(experience_kinds(&next).contains(&("sleep".to_owned(), true)));
    let (_, status) = get(root, "/app/body/api/status").await;
    assert_eq!(status["hidden_unreadable"], true);
    assert_eq!(status["days"], 2);
}

#[tokio::test]
async fn a_hide_that_couldnt_tell_search_says_so() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    // Search's work list can't be written under a file.
    fs::write(root.join("indexer"), b"").unwrap();
    let (status, saved) = set_hidden(root, &[START], true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, json!({"days": [START], "search_queued": false}));
}

#[tokio::test]
async fn hiding_a_nights_start_day_takes_its_part_after_midnight_from_the_next_day() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    let (_, open) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert_eq!(
        experience_kinds(&open),
        [("sleep".to_owned(), true), ("sleep".to_owned(), false)]
    );
    assert!(reading_names(&open).contains(&"sleep stage deep".to_owned()));
    assert_eq!(open["hidden_with"], json!([]));
    let small_hours = window_on(
        root,
        "2026-11-04T01:00:00-07:00",
        "2026-11-04T02:00:00-07:00",
    )
    .await;
    assert_eq!(small_hours["entry_total"], 2, "{small_hours}");
    assert_eq!(small_hours["events"].as_array().unwrap().len(), 1);

    set_hidden(root, &[START], true).await;
    let (_, next) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert_eq!(next["hidden"], false);
    // The next day's own night and reading stay; the hidden night's later
    // piece and its stage go, and the day says which day took them.
    assert_eq!(experience_kinds(&next), [("sleep".to_owned(), false)]);
    assert_eq!(reading_names(&next), ["heart rate"]);
    assert_eq!(
        next["hidden_with"],
        json!([{"day": START, "label": "Nov 3, 2026"}])
    );
    let small_hours = window_on(
        root,
        "2026-11-04T01:00:00-07:00",
        "2026-11-04T02:00:00-07:00",
    )
    .await;
    assert_eq!(small_hours["hidden"], false);
    assert_eq!(small_hours["has_data"], false, "{small_hours}");
    assert_eq!(small_hours["events"], json!([]));
    let evening = window_on(
        root,
        "2026-11-04T22:00:00-07:00",
        "2026-11-04T23:00:00-07:00",
    )
    .await;
    assert_eq!(evening["events"].as_array().unwrap().len(), 1, "{evening}");
    // The span on the hidden day itself says so, and holds nothing.
    let night = window_on(
        root,
        "2026-11-03T23:00:00-07:00",
        "2026-11-04T00:00:00-07:00",
    )
    .await;
    assert_eq!(night["hidden"], true);
    assert_eq!(night["has_data"], false, "{night}");
}

#[tokio::test]
async fn hiding_the_next_day_leaves_the_night_on_the_day_it_started() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    a_night_crossing_midnight(root);
    set_hidden(root, &[NEXT], true).await;
    let (_, start) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(experience_kinds(&start), [("sleep".to_owned(), false)]);
    assert_eq!(start["hidden_with"], json!([]));
    let (_, next) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert_eq!(next["hidden"], true);
    let night = window_on(
        root,
        "2026-11-03T23:00:00-07:00",
        "2026-11-04T00:00:00-07:00",
    )
    .await;
    assert_eq!(night["hidden"], false);
    assert_eq!(night["events"].as_array().unwrap().len(), 1, "{night}");
    let small_hours = window_on(
        root,
        "2026-11-04T01:00:00-07:00",
        "2026-11-04T02:00:00-07:00",
    )
    .await;
    assert_eq!(small_hours["hidden"], true);
    assert_eq!(small_hours["has_data"], false, "{small_hours}");
}

fn strava_piece(root: &Path, day: &str, key: &str, index: u64, start: &str) {
    let dir = root
        .join("chronicle")
        .join(day)
        .join("import.strava")
        .join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("workout.json"),
        json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 77,
            "tile": {"index": index, "count": 2},
            "workout": {"name": "Night Ride", "type": "Ride", "start": start, "elapsed_seconds": 1800}
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn a_workout_that_started_on_a_hidden_day_leaves_the_day_after_it() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    zone(root, "strava");
    strava_piece(root, START, "235500_300", 0, "2026-11-03T23:55:00-07:00");
    strava_piece(root, NEXT, "000000_300", 1, "2026-11-03T23:55:00-07:00");
    window(
        root,
        NEXT,
        "import.apple_health",
        "120000_3600",
        &[reading("r1", "2026-11-04 12:05:00 -0700")],
    );
    let (_, open) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert_eq!(open["strava"].as_array().unwrap().len(), 1);

    set_hidden(root, &[START], true).await;
    let (_, next) = get(root, &format!("/app/body/api/day/{NEXT}")).await;
    assert_eq!(next["strava"], json!([]), "{next}");
    assert_eq!(next["has_data"], true);
    assert_eq!(next["hidden_with"][0]["day"], START);
    let ride = window_on(
        root,
        "2026-11-04T00:00:00-07:00",
        "2026-11-04T00:10:00-07:00",
    )
    .await;
    assert_eq!(ride["has_data"], false, "{ride}");

    set_hidden(root, &[START], false).await;
    set_hidden(root, &[NEXT], true).await;
    let (_, start) = get(root, &format!("/app/body/api/day/{START}")).await;
    assert_eq!(start["strava"].as_array().unwrap().len(), 1, "{start}");
}

fn cell<'a>(status: &'a Value, day: &str) -> &'a Value {
    status["day_grid"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|year| year["weeks"].as_array().unwrap())
        .flat_map(|week| week.as_array().unwrap())
        .find(|cell| cell["day"] == day)
        .unwrap()
}

#[tokio::test]
async fn a_hidden_day_is_a_hidden_cell_and_out_of_every_count() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    for day in ["20260110", "20260112"] {
        window(
            root,
            day,
            "import.apple_health",
            "090000_3600",
            &[reading("r1", "2026-01-10 09:05:00 -0700")],
        );
    }
    window(
        root,
        "20260111",
        "import.oura",
        "000000_86400",
        &[dated("d1", "20260111")],
    );
    set_hidden(root, &["20260111"], true).await;

    let (_, status) = get(root, "/app/body/api/status").await;
    assert_eq!(status["hidden_unreadable"], false);
    assert_eq!(status["days"], 2);
    let hidden = cell(&status, "20260111");
    assert_eq!(hidden["hidden"], true);
    assert_eq!(hidden["count"], 0);
    assert_eq!(cell(&status, "20260110")["hidden"], false);
    let sources = status["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1, "{status}");
    assert_eq!(sources[0]["days"], 2);
    let (_, index) = get(root, "/app/body/api/index").await;
    assert_eq!(index["months"], json!({"202601": 2}));
    let (_, stats) = get(root, "/app/body/api/stats/202601").await;
    assert_eq!(stats, json!({"20260110": 1, "20260112": 1}));
    let (_, recent) = get(root, "/app/body/api/recent?before=20260120").await;
    let listed = recent["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["day"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(listed, ["20260112", "20260110"]);

    // The last day hidden too: coverage ends before it, and its cell stays on
    // the grid, where its unhide is reached.
    set_hidden(root, &["20260112"], true).await;
    let (_, status) = get(root, "/app/body/api/status").await;
    assert_eq!(
        status["coverage"],
        json!({"start": "20260110", "end": "20260110"})
    );
    assert_eq!(cell(&status, "20260112")["hidden"], true);
    let (_, day) = get(root, "/app/body/api/day/20260110").await;
    assert_eq!(day["nearest"]["next"], Value::Null);
}
