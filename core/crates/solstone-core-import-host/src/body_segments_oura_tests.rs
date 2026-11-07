// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods)]

use std::fs;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_retention::door;
use solstone_core_retention::tombstone::RemovalReason;

use super::*;

const STREAM: &str = "import.oura";

fn journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
    for facet in ["health", "other"] {
        let dir = root.join("facets").join(facet);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"title": facet, "description": facet}).to_string(),
        )
        .unwrap();
    }
    journal
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/importers/health/oura_pages")
}

/// Stage pages in fetch order, one per `(endpoint, documents)`, with their index.
fn stage(dir: &Path, pages: &[(&str, Vec<Value>)]) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let mut index = Vec::new();
    for (chunk, (endpoint, documents)) in pages.iter().enumerate() {
        let file = format!("{endpoint}/{chunk}-0.json");
        let bytes = json!({"data": documents, "next_token": null}).to_string();
        fs::create_dir_all(dir.join(endpoint)).unwrap();
        fs::write(dir.join(&file), &bytes).unwrap();
        let hash: String = Sha256::digest(bytes.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        index.push(
            json!({"endpoint": endpoint, "chunk": chunk, "n": 0, "file": file, "sha256": hash}),
        );
    }
    fs::write(
        dir.join("index.json"),
        json!({ "pages": index }).to_string(),
    )
    .unwrap();
    dir.to_path_buf()
}

fn request<'a>(
    pages: &'a Path,
    timestamp: &'a str,
    facet: Option<&'a str>,
) -> OuraSegmentsRequest<'a> {
    let mut request = OuraSegmentsRequest::new(pages, timestamp, facet);
    // Small enough that every phase spills into several runs.
    request.spill_bytes = 4_096;
    request
}

fn run(root: &Path, pages: &Path, timestamp: &str, facet: Option<&str>) -> OuraSegmentsOutcome {
    let outcome = run_oura_segments(root, &request(pages, timestamp, facet)).unwrap();
    assert!(outcome.finished, "{}", outcome.summary);
    outcome
}

fn window(root: &Path, day: &str, key: &str) -> PathBuf {
    root.join("chronicle").join(day).join(STREAM).join(key)
}

/// Every live window's rows, by day and key.
fn stored(root: &Path) -> BTreeMap<(String, String), Vec<BodyRow>> {
    let mut windows = BTreeMap::new();
    let Ok(days) = fs::read_dir(root.join("chronicle")) else {
        return windows;
    };
    for day in days.flatten() {
        let Ok(keys) = fs::read_dir(day.path().join(STREAM)) else {
            continue;
        };
        for key in keys.flatten() {
            let Ok(text) = fs::read_to_string(key.path().join(BODY_RECORDS_FILE)) else {
                continue;
            };
            let Ok(rows) = decode_rows(&text) else {
                continue;
            };
            windows.insert(
                (
                    day.file_name().to_string_lossy().into_owned(),
                    key.file_name().to_string_lossy().into_owned(),
                ),
                rows,
            );
        }
    }
    windows
}

/// Where the first row of Oura's document `id` is, with the row.
fn placed(root: &Path, id: &str) -> Vec<(String, String, BodyRow)> {
    let mut found = Vec::new();
    for ((day, key), rows) in stored(root) {
        for row in rows {
            if row.fields.get("id").and_then(Value::as_str) == Some(id) {
                found.push((day.clone(), key.clone(), row));
            }
        }
    }
    found
}

/// The windows of every row of `id`'s experience: its first row and its tiles.
fn spans(root: &Path, id: &str) -> Vec<(String, String)> {
    let Some((_, _, first)) = placed(root, id).into_iter().next() else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for ((day, key), rows) in stored(root) {
        if rows
            .iter()
            .any(|row| row.id == first.id || row.of.as_deref() == Some(first.id.as_str()))
        {
            windows.push((day, key));
        }
    }
    windows
}

/// Every stream file, byte for byte.
fn stream_bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let Ok(days) = fs::read_dir(root.join("chronicle")) else {
        return files;
    };
    for day in days.flatten() {
        let Ok(keys) = fs::read_dir(day.path().join(STREAM)) else {
            continue;
        };
        for key in keys.flatten() {
            for file in fs::read_dir(key.path()).into_iter().flatten().flatten() {
                if file.path().is_file() {
                    files.insert(
                        file.path()
                            .strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        fs::read(file.path()).unwrap(),
                    );
                }
            }
        }
    }
    files
}

fn delete(root: &Path, day: &str, key: &str) {
    let outcome = door::remove_segments(
        root,
        &[solstone_core_retention::Target {
            day: day.to_owned(),
            stream: STREAM.to_owned(),
            dir: key.to_owned(),
        }],
        &chrono::Utc::now().to_rfc3339(),
        RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(outcome.halted.is_none());
}

fn workout(id: &str, activity: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id, "activity": activity, "calories": 210.0, "day": &start[..10], "distance": 3100.0,
        "end_datetime": end, "intensity": "moderate", "label": null, "source": "confirmed",
        "start_datetime": start,
    })
}

fn sleep(id: &str, kind: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id, "bedtime_start": start, "bedtime_end": end, "day": &end[..10],
        "low_battery_alert": false, "period": 1, "time_in_bed": 25_000, "type": kind,
        "average_heart_rate": 55.0,
    })
}

fn session(id: &str, kind: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id, "day": &start[..10], "start_datetime": start, "end_datetime": end, "type": kind,
        "heart_rate": null, "heart_rate_variability": null, "mood": "good", "motion_count": null,
    })
}

fn tag(id: &str, start: &str) -> Value {
    json!({
        "id": id, "tag_type_code": "tag_generic_caffeine", "start_time": start, "end_time": null,
        "start_day": &start[..10], "end_day": null, "comment": "a cup", "custom_name": null,
    })
}

fn heart(at: &str, bpm: u32, source: &str) -> Value {
    json!({"timestamp": at, "bpm": bpm, "producer_timestamp": null, "source": source})
}

fn readiness(id: &str, day: &str, score: u32) -> Value {
    json!({
        "id": id, "day": day, "score": score, "timestamp": format!("{day}T00:00:00.000-07:00"),
        "contributors": {"hrv_balance": 79, "sleep_balance": 86}, "temperature_deviation": -0.2,
    })
}

#[test]
fn the_fixture_pages_land_by_the_read_table() {
    let journal = journal();
    let root = journal.path();
    let outcome = run(root, &fixture(), "20261104_120000", None);
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    let summary = &outcome.summary;
    assert!(summary.starts_with("# Oura import"), "{summary}");

    // A night that began at 22:41 lands on the day it began, though Oura's `day`
    // is the next; its tiles go on hours through the morning.
    let night = placed(root, "synthetic-sleep-2026-01-02");
    assert_eq!(night.len(), 1);
    assert_eq!(
        (night[0].0.as_str(), night[0].1.as_str()),
        ("20260101", "220000_3600")
    );
    assert_eq!(night[0].2.fields["day"], json!("2026-01-02"));
    assert_eq!(
        spans(root, "synthetic-sleep-2026-01-02").len(),
        9,
        "22:00 to 06:00"
    );

    // Workouts and sessions on the grid, a nap on hours, tags in their hour.
    let walk = &placed(root, "synthetic-workout-walk")[0];
    assert_eq!(
        (walk.0.as_str(), walk.1.as_str()),
        ("20260102", "080500_300")
    );
    let meditation = &placed(root, "synthetic-session-meditation")[0];
    assert_eq!(meditation.1, "071000_300");
    assert_eq!(meditation.2.experience.as_deref(), Some("mindfulness"));
    let nap = &placed(root, "synthetic-session-nap")[0];
    assert_eq!(nap.1, "130000_3600");
    assert_eq!(nap.2.experience.as_deref(), Some("sleep"));
    let coffee = &placed(root, "synthetic-tag-coffee")[0];
    assert_eq!(
        (coffee.1.as_str(), coffee.2.kind),
        ("090000_3600", RowKind::Logged)
    );

    // A reading inside the walk sits on the walk's tile; one outside any session
    // on its hour.
    let windows = stored(root);
    let holding = |at: &str| -> Vec<(String, String)> {
        windows
            .iter()
            .filter(|(_, rows)| rows.iter().any(|row| row.at.as_deref() == Some(at)))
            .map(|(window, _)| window.clone())
            .collect()
    };
    assert_eq!(
        holding("2026-01-02T15:20:00.000Z"),
        [("20260102".to_owned(), "082000_300".to_owned())]
    );
    assert_eq!(
        holding("2026-01-02T23:00:00.000Z"),
        [("20260102".to_owned(), "160000_3600".to_owned())]
    );

    // The eight daily documents, dated, with no value and every field.
    let dated = &windows[&("20260102".to_owned(), "000000_86400".to_owned())];
    assert_eq!(dated.len(), 8);
    for row in dated {
        assert_eq!(row.kind, RowKind::Dated);
        assert!(row.value.is_none(), "{row:?}");
        assert!(row.fields.contains_key("id"));
    }
    let stress = dated
        .iter()
        .find(|row| row.record_type == "daily_stress")
        .unwrap();
    assert_eq!(stress.fields["day_summary"], json!("stressful"));

    // What isn't read is counted and named, and none of it is stored.
    for line in [
        "- blood_glucose: 1",
        "- sleep deleted: 1",
        "- sleep rest: 1",
        "- heartrate: 3",
        "- workout: 3",
    ] {
        assert!(summary.contains(line), "{line}: {summary}");
    }
    assert!(placed(root, "synthetic-rest-2026-01-03").is_empty());
    assert!(placed(root, "synthetic-deleted-2026-01-03").is_empty());
    for rows in windows.values() {
        for row in rows {
            assert!(!row.fields.contains_key("glucose"), "{row:?}");
            assert_eq!(row.by.as_deref(), Some("Oura"));
        }
    }
    // The fixed zone is written at the first import, as Apple's is.
    assert!(root.join("imports/.oura-zone.json").is_file());
    assert!(!root.join("imports/.oura-run").exists());
}

#[test]
fn the_same_pages_again_change_nothing() {
    let journal = journal();
    let root = journal.path();
    let first = run(root, &fixture(), "20261104_120000", Some("health"));
    assert!(first.published > 10);
    let before = stream_bytes(root);
    let records = fs::read(root.join("facets/health/activities/20260102.jsonl")).unwrap();
    let again = run(root, &fixture(), "20261104_120001", Some("health"));
    assert_eq!(again.published, 0, "{}", again.summary);
    assert!(
        again.summary.contains("0 windows written"),
        "{}",
        again.summary
    );
    assert!(
        !again.summary.contains("has since changed"),
        "{}",
        again.summary
    );
    assert_eq!(stream_bytes(root), before);
    assert_eq!(
        fs::read(root.join("facets/health/activities/20260102.jsonl")).unwrap(),
        records
    );
}

#[test]
fn a_document_over_the_row_bound_is_refused_by_name_and_is_no_hole() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let mut big = workout(
        "w-big",
        "walking",
        "2026-01-05T08:00:00.000-07:00",
        "2026-01-05T08:30:00.000-07:00",
    );
    big["label"] = json!("x".repeat(40 * 1024));
    let pages = stage(
        scratch.path(),
        &[(
            "workout",
            vec![
                big,
                workout(
                    "w-small",
                    "walking",
                    "2026-01-05T10:00:00.000-07:00",
                    "2026-01-05T10:30:00.000-07:00",
                ),
            ],
        )],
    );
    let outcome = run(root, &pages, "20261104_120000", None);
    assert!(
        outcome
            .summary
            .contains("## Refused\n\n- workout: row_too_large: 1"),
        "{}",
        outcome.summary
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    assert!(placed(root, "w-big").is_empty());
    assert_eq!(placed(root, "w-small").len(), 1);
}

#[test]
fn two_versions_of_one_id_in_one_fetch_are_one_row_the_later() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let mut later = workout(
        "w1",
        "walking",
        "2026-01-05T09:00:00.000-07:00",
        "2026-01-05T09:20:00.000-07:00",
    );
    later["calories"] = json!(300.0);
    let pages = stage(
        scratch.path(),
        &[
            (
                "workout",
                vec![workout(
                    "w1",
                    "walking",
                    "2026-01-05T08:00:00.000-07:00",
                    "2026-01-05T08:20:00.000-07:00",
                )],
            ),
            ("workout", vec![later]),
        ],
    );
    run(root, &pages, "20261104_120000", None);
    let found = placed(root, "w1");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].1, "090000_300");
    assert_eq!(found[0].2.values["energy"], "300.0 kcal");
    assert!(!window(root, "20260105", "080000_300").exists());
}

#[test]
fn a_reading_oura_reclassifies_between_imports_is_one_reading_edited() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let at = "2026-01-05T17:00:00.000Z";
    run(
        root,
        &stage(
            &scratch.path().join("1"),
            &[("heartrate", vec![heart(at, 61, "rest")])],
        ),
        "20261104_120000",
        None,
    );
    let before = stored(root);
    let (window, rows) = before.iter().next().unwrap();
    let id = rows[0].id.clone();
    let second = run(
        root,
        &stage(
            &scratch.path().join("2"),
            &[("heartrate", vec![heart(at, 61, "workout")])],
        ),
        "20261104_120001",
        None,
    );
    assert_eq!(second.published, 1);
    let after = stored(root);
    assert_eq!(after.len(), 1);
    let rows = &after[window];
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].fields["source"], json!("workout"));
    // The first import still owns it.
    assert_eq!(rows[0].run.as_deref(), Some("20261104_120000"));
}

#[test]
fn two_real_sources_at_one_instant_in_one_fetch_are_two_rows() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let at = "2026-01-05T17:00:00.000Z";
    run(
        root,
        &stage(
            scratch.path(),
            &[(
                "heartrate",
                vec![heart(at, 61, "awake"), heart(at, 74, "live")],
            )],
        ),
        "20261104_120000",
        None,
    );
    let rows = &stored(root)[&("20260105".to_owned(), "100000_3600".to_owned())];
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].id, rows[1].id);
}

#[test]
fn an_oura_reading_inside_an_oura_workout_sits_on_its_tile_and_inside_a_night_on_its_hour() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = stage(
        scratch.path(),
        &[
            (
                "workout",
                vec![workout(
                    "w1",
                    "running",
                    "2026-01-05T10:00:00.000-07:00",
                    "2026-01-05T10:30:00.000-07:00",
                )],
            ),
            (
                "sleep",
                vec![sleep(
                    "n1",
                    "long_sleep",
                    "2026-01-05T22:00:00.000-07:00",
                    "2026-01-06T06:00:00.000-07:00",
                )],
            ),
            (
                "heartrate",
                vec![
                    heart("2026-01-05T17:12:00.000Z", 141, "workout"),
                    heart("2026-01-06T08:17:00.000Z", 49, "sleep"),
                ],
            ),
        ],
    );
    run(root, &pages, "20261104_120000", None);
    let windows = stored(root);
    let reading_in = |day: &str, key: &str| {
        windows
            .get(&(day.to_owned(), key.to_owned()))
            .is_some_and(|rows| rows.iter().any(|row| row.kind == RowKind::Reading))
    };
    assert!(reading_in("20260105", "101000_300"));
    assert!(!reading_in("20260105", "100000_3600"));
    assert!(reading_in("20260106", "010000_3600"));
}

#[test]
fn a_revised_score_is_an_edit_of_its_dated_row() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    run(
        root,
        &stage(
            &scratch.path().join("1"),
            &[("daily_readiness", vec![readiness("r1", "2026-01-05", 82)])],
        ),
        "20261104_120000",
        None,
    );
    let outcome = run(
        root,
        &stage(
            &scratch.path().join("2"),
            &[("daily_readiness", vec![readiness("r1", "2026-01-05", 79)])],
        ),
        "20261104_120001",
        None,
    );
    assert_eq!(outcome.published, 1);
    let found = placed(root, "r1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "000000_86400");
    assert_eq!(found[0].2.fields["score"], json!(79));
    assert!(found[0].2.value.is_none());
}

fn night_pages(dir: &Path, end: &str) -> PathBuf {
    stage(
        dir,
        &[(
            "sleep",
            vec![sleep(
                "n1",
                "long_sleep",
                "2026-01-05T22:10:00.000-07:00",
                end,
            )],
        )],
    )
}

#[test]
fn a_later_end_grows_a_night_and_an_earlier_end_within_its_last_tile_is_an_edit() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    run(
        root,
        &night_pages(&scratch.path().join("1"), "2026-01-06T05:20:00.000-07:00"),
        "20261104_120000",
        None,
    );
    assert_eq!(spans(root, "n1").len(), 8, "22:00 to 05:00");
    let grown = run(
        root,
        &night_pages(&scratch.path().join("2"), "2026-01-06T07:40:00.000-07:00"),
        "20261104_120001",
        None,
    );
    assert!(
        !grown.summary.contains("has since changed"),
        "{}",
        grown.summary
    );
    assert_eq!(spans(root, "n1").len(), 10, "22:00 to 07:00");
    assert_eq!(
        placed(root, "n1")[0].2.end.as_deref(),
        Some("2026-01-06T07:40:00.000-07:00")
    );
    // Every tile states the whole new span.
    for rows in stored(root).values() {
        for row in rows
            .iter()
            .filter(|row| row.kind == RowKind::ExperienceTile)
        {
            assert_eq!(row.end.as_deref(), Some("2026-01-06T07:40:00.000-07:00"));
            assert_eq!(row.count, Some(10));
        }
    }
    let shorter = run(
        root,
        &night_pages(&scratch.path().join("3"), "2026-01-06T07:05:00.000-07:00"),
        "20261104_120002",
        None,
    );
    assert!(
        !shorter.summary.contains("has since changed"),
        "{}",
        shorter.summary
    );
    assert!(shorter.holes.is_empty());
    assert_eq!(spans(root, "n1").len(), 10);
    assert_eq!(
        placed(root, "n1")[0].2.end.as_deref(),
        Some("2026-01-06T07:05:00.000-07:00")
    );
}

#[test]
fn an_earlier_end_marks_the_tiles_it_leaves_outside_on_the_next_day() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    run(
        root,
        &night_pages(&scratch.path().join("1"), "2026-01-06T07:40:00.000-07:00"),
        "20261104_120000",
        None,
    );
    let rid = oura_row_id("sleep", "n1");
    let before = rows_of(root, &rid);
    let outcome = run(
        root,
        &night_pages(&scratch.path().join("2"), "2026-01-06T05:20:00.000-07:00"),
        "20261104_120001",
        None,
    );
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    assert!(
        outcome
            .summary
            .contains("Oura has since changed the time of 1 entry"),
        "{}",
        outcome.summary
    );
    let mut windows = windows_from("20260105", (22, 0), 3600, 2);
    windows.extend(windows_from("20260106", (0, 0), 3600, 6));
    assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
    // The two hours it no longer reaches hold only their marks.
    for key in ["060000_3600", "070000_3600"] {
        let rows = &stored(root)[&("20260106".to_owned(), key.to_owned())];
        assert!(
            rows.iter().all(|row| row.kind == RowKind::Moved),
            "{rows:?}"
        );
    }
}
/// The row id the writer gives Oura's document `id` from `endpoint`.
fn oura_row_id(endpoint: &str, id: &str) -> String {
    Sha256::digest(format!("oura\u{1f}{endpoint}\u{1f}{id}\u{1f}0").as_bytes())[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A place in the stream: day, window and row id.
type Place = (String, String, String);

/// Every row of a document across the stream, live or moved: its first rows, its
/// tiles and its marks.
fn rows_of(root: &Path, rid: &str) -> BTreeMap<Place, BodyRow> {
    let mut found = BTreeMap::new();
    for ((day, key), rows) in stored(root) {
        for row in rows {
            if row.id == rid || row.of.as_deref() == Some(rid) {
                found.insert((day.clone(), key.clone(), row.id.clone()), row);
            }
        }
    }
    found
}

/// A document's tiling over these windows: its first row in the first, and a tile
/// named by its window in each of the others.
fn tiling(rid: &str, windows: &[(&str, &str)]) -> BTreeSet<Place> {
    windows
        .iter()
        .enumerate()
        .map(|(index, (day, key))| {
            let id = if index == 0 {
                rid.to_owned()
            } else {
                format!("{rid}@{day}/{key}")
            };
            ((*day).to_owned(), (*key).to_owned(), id)
        })
        .collect()
}

/// Windows: `count` of `length` seconds on `day`, from `clock`.
fn windows_from(day: &str, clock: (u32, u32), length: u32, count: u32) -> Vec<(String, String)> {
    (0..count)
        .map(|step| {
            let seconds = clock.0 * 3600 + clock.1 * 60 + step * length;
            (
                day.to_owned(),
                format!(
                    "{:02}{:02}{:02}_{length}",
                    seconds / 3600,
                    (seconds % 3600) / 60,
                    seconds % 60
                ),
            )
        })
        .collect()
}

fn as_refs(windows: &[(String, String)]) -> Vec<(&str, &str)> {
    windows
        .iter()
        .map(|(day, key)| (day.as_str(), key.as_str()))
        .collect()
}

/// After a run that wrote every day of a document, its live rows across the
/// stream are exactly `expected`, and every live row it had before that isn't is
/// now `moved`: that row's id byte for byte, its document as `of`, its run, source,
/// type and label, and nothing else.
fn assert_converged(
    root: &Path,
    rid: &str,
    before: &BTreeMap<Place, BodyRow>,
    expected: &BTreeSet<Place>,
) {
    let after = rows_of(root, rid);
    let live: BTreeSet<Place> = after
        .iter()
        .filter(|(_, row)| row.kind != RowKind::Moved)
        .map(|(place, _)| place.clone())
        .collect();
    assert_eq!(&live, expected, "{after:#?}");
    for (place, old) in before {
        if old.kind == RowKind::Moved || expected.contains(place) {
            continue;
        }
        let mut mark = BodyRow::new(
            RowKind::Moved,
            old.id.clone(),
            &old.source,
            &old.record_type,
            old.label.clone(),
        );
        mark.of = Some(rid.to_owned());
        mark.run.clone_from(&old.run);
        assert_eq!(after.get(place), Some(&mark), "{place:?}: {after:#?}");
    }
}

#[test]
fn a_start_across_the_owners_midnight_is_kept_as_first_written_and_within_its_window_is_an_edit() {
    for (first, revised) in [
        // 00:10 to 23:40 the day before, and 23:40 to 00:10 the day after.
        (
            (
                "2026-01-05T00:10:00.000-07:00",
                "2026-01-05T00:40:00.000-07:00",
            ),
            (
                "2026-01-04T23:40:00.000-07:00",
                "2026-01-05T00:40:00.000-07:00",
            ),
        ),
        (
            (
                "2026-01-05T23:40:00.000-07:00",
                "2026-01-06T00:20:00.000-07:00",
            ),
            (
                "2026-01-06T00:10:00.000-07:00",
                "2026-01-06T00:20:00.000-07:00",
            ),
        ),
    ] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let pages = |name: &str, (start, end): (&str, &str)| {
            stage(
                &scratch.path().join(name),
                &[("workout", vec![workout("w1", "running", start, end)])],
            )
        };
        run(root, &pages("1", first), "20261104_120000", None);
        let before = stream_bytes(root);
        let outcome = run(root, &pages("2", revised), "20261104_120001", None);
        assert_eq!(stream_bytes(root), before, "{revised:?}");
        assert!(
            outcome
                .summary
                .contains("Oura has since changed 1 entry; the journal keeps it as first written."),
            "{}",
            outcome.summary
        );
        assert!(
            !outcome.summary.contains("now shows"),
            "{}",
            outcome.summary
        );
        assert_eq!(
            outcome.holes,
            [Hole {
                endpoint: "workout".to_owned(),
                at: revised.0[..10].to_owned(),
                reason: HoleReason::Deferred,
            }]
        );
        assert!(
            outcome.summary.contains(&format!(
                "- workout, {}: changed in Oura since",
                &revised.0[..10]
            )),
            "{}",
            outcome.summary
        );
    }
    // A start moved within its first window is an edit, and no move.
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let at = |start: &str| {
        vec![workout(
            "w1",
            "running",
            &format!("2026-01-05T{start}:00.000-07:00"),
            "2026-01-05T08:30:00.000-07:00",
        )]
    };
    run(
        root,
        &stage(&scratch.path().join("1"), &[("workout", at("08:00"))]),
        "20261104_120000",
        None,
    );
    let within = run(
        root,
        &stage(&scratch.path().join("2"), &[("workout", at("08:02"))]),
        "20261104_120001",
        None,
    );
    assert!(
        !within.summary.contains("has since changed"),
        "{}",
        within.summary
    );
    assert!(within.holes.is_empty());
    let rid = oura_row_id("workout", "w1");
    assert!(
        rows_of(root, &rid)
            .values()
            .all(|row| row.kind != RowKind::Moved)
    );
    assert_eq!(
        placed(root, "w1")[0].2.start.as_deref(),
        Some("2026-01-05T08:02:00.000-07:00")
    );
}
#[test]
fn a_tag_moved_to_another_hour_moves_there_and_within_its_hour_is_an_edit() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, start: &str| {
        stage(
            &scratch.path().join(name),
            &[("enhanced_tag", vec![tag("t1", start)])],
        )
    };
    run(
        root,
        &pages("1", "2026-01-05T09:15:00-07:00"),
        "20261104_120000",
        None,
    );
    let rid = oura_row_id("enhanced_tag", "t1");
    let before = rows_of(root, &rid);
    let moved = run(
        root,
        &pages("2", "2026-01-05T11:15:00-07:00"),
        "20261104_120001",
        None,
    );
    assert!(moved.holes.is_empty(), "{:?}", moved.holes);
    assert!(
        moved.summary.contains(
            "Oura has since changed the time of 1 entry; the journal now shows it at the new time."
        ),
        "{}",
        moved.summary
    );
    assert_converged(
        root,
        &rid,
        &before,
        &tiling(&rid, &[("20260105", "110000_3600")]),
    );
    // No record: a tag has none.
    assert!(!root.join("facets/health/activities").exists());
    let within = run(
        root,
        &pages("3", "2026-01-05T11:45:00-07:00"),
        "20261104_120002",
        None,
    );
    assert!(within.holes.is_empty());
    assert!(!within.summary.contains("has since changed"));
    let found = placed(root, "t1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "110000_3600");
    assert_eq!(
        found[0].2.start.as_deref(),
        Some("2026-01-05T11:45:00-07:00")
    );
}
#[test]
fn a_meditation_that_becomes_a_nap_moves_to_its_hour() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, kind: &str| {
        stage(
            &scratch.path().join(name),
            &[(
                "session",
                vec![session(
                    "s1",
                    kind,
                    "2026-01-05T13:00:00.000-07:00",
                    "2026-01-05T13:20:00.000-07:00",
                )],
            )],
        )
    };
    run(
        root,
        &pages("1", "meditation"),
        "20261104_120000",
        Some("health"),
    );
    let rid = oura_row_id("session", "s1");
    let before = rows_of(root, &rid);
    assert_eq!(before.len(), 4, "13:00 to 13:20 on the grid");
    let outcome = run(root, &pages("2", "nap"), "20261104_120001", Some("health"));
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
    assert_converged(
        root,
        &rid,
        &before,
        &tiling(&rid, &[("20260105", "130000_3600")]),
    );
    // Its one record follows: a nap is sleep.
    let records = fs::read_to_string(root.join("facets/health/activities/20260105.jsonl")).unwrap();
    assert_eq!(records.lines().count(), 1, "{records}");
    assert!(records.contains("\"activity\":\"sleep\""), "{records}");
    // A type that keeps the window's length is an edit.
    let breathing = run(
        root,
        &pages("3", "breathing"),
        "20261104_120002",
        Some("health"),
    );
    assert!(breathing.holes.is_empty());
    assert_eq!(placed(root, "s1")[0].2.label, "breathing session");
}

fn one_workout(dir: &Path, start: &str, end: &str) -> PathBuf {
    stage(
        dir,
        &[("workout", vec![workout("w1", "running", start, end)])],
    )
}

#[test]
fn an_owner_deleted_window_with_a_record_stays_deleted_where_it_was_or_moved() {
    for moved in [false, true] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        run(
            root,
            &one_workout(
                &scratch.path().join("1"),
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:20:00.000-07:00",
            ),
            "20261104_120000",
            Some("health"),
        );
        assert!(
            root.join("facets/health/activities/20260105.jsonl")
                .is_file()
        );
        delete(root, "20260105", "080000_300");
        let (start, end) = if moved {
            (
                "2026-01-05T08:40:00.000-07:00",
                "2026-01-05T09:00:00.000-07:00",
            )
        } else {
            (
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:20:00.000-07:00",
            )
        };
        let outcome = run(
            root,
            &one_workout(&scratch.path().join("2"), start, end),
            "20261104_120001",
            Some("health"),
        );
        assert!(
            outcome
                .summary
                .contains("1 entry wasn't added, because you deleted"),
            "{moved}: {}",
            outcome.summary
        );
        assert!(!outcome.summary.contains("has since changed"));
        // The owner's delete is no hole: asking Oura again would never fill it.
        assert!(outcome.holes.is_empty(), "{moved}: {:?}", outcome.holes);
        assert!(placed(root, "w1").is_empty(), "{moved}");
        assert!(!window(root, "20260105", "084000_300").exists());
        assert!(
            window(root, "20260105", "080000_300")
                .join("tombstone.json")
                .is_file()
        );
    }
}

#[test]
fn with_no_record_a_deleted_window_on_the_lookup_days_refuses_a_revision_by_its_day() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, start: &str| {
        stage(
            &scratch.path().join(name),
            &[("enhanced_tag", vec![tag("t1", start)])],
        )
    };
    run(
        root,
        &pages("1", "2026-01-05T09:15:00-07:00"),
        "20261104_120000",
        None,
    );
    delete(root, "20260105", "090000_3600");
    let outcome = run(
        root,
        &pages("2", "2026-01-05T11:15:00-07:00"),
        "20261104_120001",
        None,
    );
    assert!(placed(root, "t1").is_empty());
    assert!(!window(root, "20260105", "110000_3600").exists());
    // Refused, naming the deleted window's day, and no hole: re-reading never
    // fills it.
    assert!(
        outcome.summary.contains(
            "## Refused\n\n- enhanced_tag: 1 not added; the journal can't tell whether it was in what you deleted from 2026-01-05"
        ),
        "{}",
        outcome.summary
    );
    assert!(!outcome.summary.contains("has since changed"));
    assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);

    // The twin: the same tag unchanged, into the hour the owner deleted, stays
    // deleted, and says nothing of Oura changing it.
    let again = run(
        root,
        &pages("3", "2026-01-05T09:15:00-07:00"),
        "20261104_120002",
        None,
    );
    assert!(placed(root, "t1").is_empty());
    assert!(
        !again.summary.contains("Oura has since changed"),
        "{}",
        again.summary
    );
    assert!(
        again.summary.contains("1 entry wasn't added"),
        "{}",
        again.summary
    );
    assert!(again.holes.is_empty(), "{:?}", again.holes);
}
#[test]
fn a_window_on_the_lookup_days_that_isnt_rows_holds_a_document_back_and_its_twin_lands() {
    for readable in [false, true] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let other = window(root, "20260104", "030000_3600");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join(BODY_RECORDS_FILE),
            if readable { "" } else { "not rows\n" },
        )
        .unwrap();
        let outcome = run(
            root,
            &one_workout(
                scratch.path(),
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:20:00.000-07:00",
            ),
            "20261104_120000",
            None,
        );
        if readable {
            assert_eq!(placed(root, "w1").len(), 1);
            assert!(outcome.holes.is_empty());
            assert!(!outcome.summary.contains("Held back"));
        } else {
            assert!(placed(root, "w1").is_empty());
            assert!(
                outcome.summary.contains("## Held back")
                    && outcome.summary.contains("- workout: 1"),
                "{}",
                outcome.summary
            );
            assert_eq!(
                outcome.holes,
                [Hole {
                    endpoint: "workout".to_owned(),
                    at: "2026-01-05".to_owned(),
                    reason: HoleReason::HeldBack,
                }]
            );
            assert_eq!(
                fs::read_to_string(other.join(BODY_RECORDS_FILE)).unwrap(),
                "not rows\n"
            );
        }
    }
}

#[test]
fn a_deleted_or_rejected_sleep_isnt_read_and_a_night_stored_under_it_is_named_by_date() {
    for (kind, words) in [
        ("deleted", "marks it deleted"),
        ("rest", "marks it as not sleep"),
    ] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let pages = |name: &str, kind: &str| {
            stage(
                &scratch.path().join(name),
                &[(
                    "sleep",
                    vec![sleep(
                        "n1",
                        kind,
                        "2026-01-05T22:10:00.000-07:00",
                        "2026-01-06T05:20:00.000-07:00",
                    )],
                )],
            )
        };
        run(root, &pages("1", "long_sleep"), "20261104_120000", None);
        let before = stream_bytes(root);
        let outcome = run(root, &pages("2", kind), "20261104_120001", None);
        assert_eq!(stream_bytes(root), before, "{kind}");
        assert!(
            outcome.summary.contains(&format!(
                "The sleep stored for 2026-01-05 stays as first written, though Oura now {words}."
            )),
            "{kind}: {}",
            outcome.summary
        );
        assert!(outcome.summary.contains(&format!("- sleep {kind}: 1")));
        assert!(outcome.holes.is_empty());
        // Without a stored night there's nothing to name.
        let fresh = journal_with_nothing();
        let none = run(fresh.path(), &pages("3", kind), "20261104_120002", None);
        assert!(
            !none.summary.contains("The sleep stored"),
            "{}",
            none.summary
        );
    }
}

fn journal_with_nothing() -> tempfile::TempDir {
    journal()
}

#[test]
fn oura_experiences_get_records_with_a_facet_and_a_retyped_workouts_record_follows() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = |name: &str, activity: &str| {
        stage(
            &scratch.path().join(name),
            &[
                (
                    "workout",
                    vec![workout(
                        "w1",
                        activity,
                        "2026-01-05T08:00:00.000-07:00",
                        "2026-01-05T08:20:00.000-07:00",
                    )],
                ),
                (
                    "session",
                    vec![session(
                        "s1",
                        "meditation",
                        "2026-01-05T12:00:00.000-07:00",
                        "2026-01-05T12:10:00.000-07:00",
                    )],
                ),
                (
                    "sleep",
                    vec![sleep(
                        "n1",
                        "long_sleep",
                        "2026-01-05T22:10:00.000-07:00",
                        "2026-01-06T05:20:00.000-07:00",
                    )],
                ),
                ("enhanced_tag", vec![tag("t1", "2026-01-05T09:15:00-07:00")]),
            ],
        )
    };
    let first = run(
        root,
        &pages("1", "walking"),
        "20261104_120000",
        Some("health"),
    );
    assert!(
        first.summary.contains(
            "3 of the workouts, nights of sleep and mindfulness sessions now show on their days."
        ),
        "{}",
        first.summary
    );
    let file = root.join("facets/health/activities/20260105.jsonl");
    let records: Vec<Value> = fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 3, "a tag gets no record");
    let walk = records
        .iter()
        .find(|record| record["activity"] == "workout")
        .unwrap();
    assert_eq!(walk["account"]["type"], "walking");
    assert!(
        walk["account"]["key"]
            .as_str()
            .unwrap()
            .starts_with("import.oura\u{1f}")
    );

    let retyped = run(
        root,
        &pages("2", "running"),
        "20261104_120001",
        Some("health"),
    );
    assert!(
        retyped.summary.contains("changed its times or type"),
        "{}",
        retyped.summary
    );
    let after: Vec<Value> = fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(after.len(), 3);
    let run_record = after
        .iter()
        .find(|record| record["activity"] == "workout")
        .unwrap();
    assert_eq!(run_record["account"]["type"], "running");
    assert_eq!(run_record["id"], walk["id"]);

    // Without a facet, none, and the summary says why.
    let fresh = journal_with_nothing();
    let bare = run(
        fresh.path(),
        &pages("3", "walking"),
        "20261104_120002",
        None,
    );
    assert!(
        bare.summary.contains("3 of the workouts, nights of sleep and mindfulness sessions don't show on their days, because their windows have no facet."),
        "{}",
        bare.summary
    );
    assert!(!fresh.path().join("facets/health/activities").exists());
}

#[test]
fn a_workout_at_midnight_steps_around_a_retired_card_and_a_value_revision_there_is_an_edit() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let card = window(root, "20260105", "000000_300");
    fs::create_dir_all(&card).unwrap();
    fs::write(card.join("day_summary_transcript.md"), "a card\n").unwrap();
    let pages = |name: &str, calories: f64| {
        let mut document = workout(
            "w1",
            "running",
            "2026-01-05T00:00:00.000-07:00",
            "2026-01-05T00:20:00.000-07:00",
        );
        document["calories"] = json!(calories);
        stage(&scratch.path().join(name), &[("workout", vec![document])])
    };
    run(root, &pages("1", 210.0), "20261104_120000", None);
    let found = placed(root, "w1");
    assert_eq!(found[0].1, "000000_301");
    assert!(!card.join(BODY_RECORDS_FILE).exists());
    let edited = run(root, &pages("2", 260.0), "20261104_120001", None);
    assert!(edited.holes.is_empty(), "{:?}", edited.holes);
    assert!(!edited.summary.contains("has since changed"));
    let found = placed(root, "w1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "000000_301");
    assert_eq!(found[0].2.values["energy"], "260.0 kcal");
}

#[test]
fn a_revision_kept_as_first_written_pulls_no_reading_onto_its_new_slot() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    run(
        root,
        &one_workout(
            &scratch.path().join("1"),
            "2026-01-05T00:10:00.000-07:00",
            "2026-01-05T00:40:00.000-07:00",
        ),
        "20261104_120000",
        None,
    );
    // Revised to begin half an hour earlier, across midnight, with a reading in
    // the new half hour.
    let pages = stage(
        &scratch.path().join("2"),
        &[
            (
                "workout",
                vec![workout(
                    "w1",
                    "running",
                    "2026-01-04T23:40:00.000-07:00",
                    "2026-01-05T00:40:00.000-07:00",
                )],
            ),
            (
                "heartrate",
                vec![heart("2026-01-05T06:50:00.000Z", 150, "workout")],
            ),
        ],
    );
    let outcome = run(root, &pages, "20261104_120001", None);
    assert!(outcome.summary.contains("Oura has since changed 1 entry"));
    let windows = stored(root);
    assert!(!windows.contains_key(&("20260104".to_owned(), "235000_300".to_owned())));
    assert!(!windows.contains_key(&("20260104".to_owned(), "234000_300".to_owned())));
    let hour = &windows[&("20260104".to_owned(), "230000_3600".to_owned())];
    assert_eq!(hour.len(), 1);
    assert_eq!(hour[0].kind, RowKind::Reading);
}

/// Every file that says what the journal holds, byte for byte: the stream, the
/// activity records (without the journal's own destination id), and each import's
/// summary, listing and manifest totals.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = stream_bytes(root);
    for facet in fs::read_dir(root.join("facets"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for file in fs::read_dir(facet.path().join("activities"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if !file.path().is_file() || file.path().extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let relative = file
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let mut text = String::new();
            for line in fs::read_to_string(file.path()).unwrap().lines() {
                let mut record: Value = serde_json::from_str(line).unwrap();
                record.as_object_mut().unwrap().remove("destination_id");
                text.push_str(&format!("{record}\n"));
            }
            files.insert(relative, text.into_bytes());
        }
    }
    for import in fs::read_dir(root.join("imports")).unwrap().flatten() {
        let dir = import.path();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        for name in ["summary.md", "body-touched.jsonl"] {
            if let Ok(bytes) = fs::read(dir.join(name)) {
                files.insert(format!("imports/{id}/{name}"), bytes);
            }
        }
        if let Ok(bytes) = fs::read(dir.join("manifest.json")) {
            let manifest: Value = serde_json::from_slice(&bytes).unwrap();
            files.insert(
                format!("imports/{id}/manifest totals"),
                json!([manifest["entry_count"], manifest["days_affected"]])
                    .to_string()
                    .into_bytes(),
            );
        }
    }
    files
}

/// The days an import wrote, in the order it wrote them, from its listing.
fn written_days(root: &Path, import_id: &str) -> Vec<String> {
    fs::read_to_string(root.join("imports").join(import_id).join(LISTING_FILE))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["day"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn stopping_at(point: &'static str, nth: usize) -> impl Fn(&str) -> bool {
    let seen = std::cell::Cell::new(0);
    move |name| {
        if name == point {
            seen.set(seen.get() + 1);
            seen.get() == nth
        } else {
            false
        }
    }
}

/// A journal holding a first import, and windows that can't be read as rows where
/// the second import's documents and readings would look or go.
fn kill_base(root: &Path, scratch: &Path) {
    let first = stage(
        &scratch.join("base"),
        &[
            (
                "workout",
                vec![
                    workout(
                        "w-moved",
                        "running",
                        "2026-01-06T08:00:00.000-07:00",
                        "2026-01-06T08:30:00.000-07:00",
                    ),
                    workout(
                        "w-deferred",
                        "running",
                        "2026-01-10T00:10:00.000-07:00",
                        "2026-01-10T00:40:00.000-07:00",
                    ),
                ],
            ),
            (
                "sleep",
                vec![
                    sleep(
                        "n-grown",
                        "long_sleep",
                        "2026-01-06T22:10:00.000-07:00",
                        "2026-01-07T05:20:00.000-07:00",
                    ),
                    sleep(
                        "n-moved",
                        "long_sleep",
                        "2026-01-08T22:10:00.000-07:00",
                        "2026-01-09T06:30:00.000-07:00",
                    ),
                ],
            ),
        ],
    );
    run(root, &first, "20261104_110000", Some("health"));
    let garbage = |day: &str, key: &str| {
        let dir = window(root, day, key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(BODY_RECORDS_FILE), "not rows\n").unwrap();
    };
    // Where the new workout could already be: it's held back.
    garbage("20260112", "030000_3600");
    // Where a reading goes: its window is left alone.
    garbage("20260118", "100000_3600");
    // A reading's other place: it's held back.
    garbage("20260118", "142000_300");
    // A day whose activity file can't be locked refuses its records.
    let activities = root.join("facets/health/activities");
    fs::create_dir_all(activities.join("20260124.jsonl.lock")).unwrap();
}

fn kill_pages(scratch: &Path) -> PathBuf {
    stage(
        &scratch.join("revised"),
        &[
            (
                "workout",
                vec![
                    workout(
                        "w-moved",
                        "running",
                        "2026-01-06T08:40:00.000-07:00",
                        "2026-01-06T09:10:00.000-07:00",
                    ),
                    workout(
                        "w-held",
                        "walking",
                        "2026-01-12T10:00:00.000-07:00",
                        "2026-01-12T10:30:00.000-07:00",
                    ),
                    workout(
                        "w-unrecorded",
                        "walking",
                        "2026-01-24T08:00:00.000-07:00",
                        "2026-01-24T08:20:00.000-07:00",
                    ),
                    // Across the owner's midnight: kept as first written.
                    workout(
                        "w-deferred",
                        "running",
                        "2026-01-09T23:40:00.000-07:00",
                        "2026-01-10T00:40:00.000-07:00",
                    ),
                ],
            ),
            (
                "sleep",
                vec![
                    sleep(
                        "n-grown",
                        "long_sleep",
                        "2026-01-06T22:10:00.000-07:00",
                        "2026-01-07T07:40:00.000-07:00",
                    ),
                    // A two-day night whose start moves within its day.
                    sleep(
                        "n-moved",
                        "long_sleep",
                        "2026-01-08T23:10:00.000-07:00",
                        "2026-01-09T06:30:00.000-07:00",
                    ),
                ],
            ),
            (
                "heartrate",
                vec![
                    heart("2026-01-07T01:00:00.000Z", 58, "awake"),
                    heart("2026-01-18T17:20:00.000Z", 70, "awake"),
                    heart("2026-01-18T19:00:00.000Z", 71, "awake"),
                    heart("2026-01-18T21:20:00.000Z", 72, "awake"),
                ],
            ),
            ("daily_readiness", vec![readiness("r1", "2026-01-07", 80)]),
            ("daily_readiness", vec![readiness("r2", "2026-01-20", 77)]),
        ],
    )
}

#[test]
fn an_oura_run_stopped_anywhere_resumes_to_what_an_uninterrupted_run_writes_with_its_holes() {
    let scratch = tempfile::tempdir().unwrap();
    let whole = journal();
    kill_base(whole.path(), scratch.path());
    let pages = kill_pages(scratch.path());
    let done = run_oura_segments(
        whole.path(),
        &request(&pages, "20261104_130000", Some("health")),
    )
    .unwrap();
    assert!(done.finished, "{}", done.summary);
    let expected = snapshot(whole.path());
    let reasons: BTreeSet<HoleReason> = done.holes.iter().map(|hole| hole.reason).collect();
    assert_eq!(
        reasons,
        BTreeSet::from([
            HoleReason::Deferred,
            HoleReason::HeldBack,
            HoleReason::Unreadable,
            HoleReason::RecordRefused,
            HoleReason::ReadingHeldBack,
        ]),
        "{:?}",
        done.holes
    );
    assert!(done.holes.contains(&Hole {
        endpoint: "heartrate".to_owned(),
        at: "2026-01-18T17:00:00Z".to_owned(),
        reason: HoleReason::Unreadable,
    }));
    assert!(done.holes.contains(&Hole {
        endpoint: "heartrate".to_owned(),
        at: "2026-01-18T21:00:00Z".to_owned(),
        reason: HoleReason::ReadingHeldBack,
    }));
    assert!(done.holes.contains(&Hole {
        endpoint: "workout".to_owned(),
        at: "2026-01-24".to_owned(),
        reason: HoleReason::RecordRefused,
    }));

    // The two-day night converges on its start day and the next: either side of
    // each, and between the passes of the days that have two.
    let order = written_days(whole.path(), &done.import_id);
    let at = |day: &str| order.iter().position(|written| written == day).unwrap() + 1;
    for (point, nth) in [
        ("scanned", 1),
        ("placed", 1),
        ("pending", 1),
        ("pending", 3),
        ("written", 1),
        ("written", 2),
        ("marking", 1),
        ("marking", 2),
        ("published", 1),
        ("published", 4),
        ("pending", at("20260108")),
        ("published", at("20260108")),
        ("pending", at("20260109")),
        ("published", at("20260109")),
        ("written-all", 1),
        ("finished", 1),
    ] {
        let journal = journal();
        let root = journal.path();
        kill_base(root, scratch.path());
        let stop = stopping_at(point, nth);
        let mut stopped = request(&pages, "20261104_130000", Some("health"));
        stopped.stop_at = Some(&stop);
        let error = run_oura_segments(root, &stopped).unwrap_err();
        assert!(error.contains(point), "{point} {nth}: {error}");
        let resumed =
            run_oura_segments(root, &request(&pages, "20261104_140000", Some("health"))).unwrap();
        assert!(resumed.finished, "{point} {nth}: {}", resumed.summary);
        assert_eq!(resumed.import_id, done.import_id, "{point} {nth}");
        assert!(!root.join("imports/20261104_140000").exists());
        assert!(!root.join(OURA.run_dir_rel).exists());
        assert_eq!(resumed.holes, done.holes, "{point} {nth}");
        let got = snapshot(root);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "{point} {nth}"
        );
        for (path, bytes) in &expected {
            assert_eq!(got.get(path), Some(bytes), "{point} {nth}: {path}");
        }
    }
}

#[test]
fn a_takeover_by_other_pages_carries_the_stopped_runs_holes() {
    let scratch = tempfile::tempdir().unwrap();
    let other = stage(
        &scratch.path().join("other"),
        &[("daily_readiness", vec![readiness("r9", "2026-02-01", 70)])],
    );
    // Stopped once the scan's holes are saved, and once the fifth day, whose
    // readings couldn't all be written, is pending.
    for (point, nth) in [("placed", 1), ("pending", 5)] {
        let journal = journal();
        let root = journal.path();
        kill_base(root, scratch.path());
        let pages = kill_pages(scratch.path());
        let stop = stopping_at(point, nth);
        let mut stopped = request(&pages, "20261104_130000", Some("health"));
        stopped.stop_at = Some(&stop);
        run_oura_segments(root, &stopped).unwrap_err();
        let state: Value = serde_json::from_slice(
            &fs::read(root.join(OURA.run_dir_rel).join(STATE_FILE)).unwrap(),
        )
        .unwrap();
        let taken = run(root, &other, "20261104_140000", Some("health"));
        assert_ne!(taken.import_id, "20261104_130000");
        for hole in [
            Hole {
                endpoint: "workout".to_owned(),
                at: "2026-01-09".to_owned(),
                reason: HoleReason::Deferred,
            },
            Hole {
                endpoint: "workout".to_owned(),
                at: "2026-01-12".to_owned(),
                reason: HoleReason::HeldBack,
            },
            // A move the stopped run hadn't finished stays a hole of its own.
            Hole {
                endpoint: "workout".to_owned(),
                at: "2026-01-06".to_owned(),
                reason: HoleReason::Converging,
            },
        ] {
            assert!(
                taken.holes.contains(&hole),
                "{point}: {:?} {state}",
                taken.holes
            );
        }
        if point == "pending" {
            assert_eq!(state["pending"]["day"], "20260118", "{state}");
            for hole in [
                Hole {
                    endpoint: "heartrate".to_owned(),
                    at: "2026-01-18T17:00:00Z".to_owned(),
                    reason: HoleReason::Unreadable,
                },
                Hole {
                    endpoint: "heartrate".to_owned(),
                    at: "2026-01-18T21:00:00Z".to_owned(),
                    reason: HoleReason::ReadingHeldBack,
                },
            ] {
                assert!(taken.holes.contains(&hole), "{:?} {state}", taken.holes);
            }
        }
    }
}

#[test]
fn a_page_whose_bytes_arent_what_its_index_records_saves_nothing() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let pages = one_workout(
        scratch.path(),
        "2026-01-05T08:00:00.000-07:00",
        "2026-01-05T08:20:00.000-07:00",
    );
    let page = pages.join("workout/0-0.json");
    let text = fs::read_to_string(&page).unwrap().replace("210.0", "999.0");
    fs::write(&page, text).unwrap();
    let error = run_oura_segments(root, &request(&pages, "20261104_120000", None)).unwrap_err();
    assert!(error.contains("doesn't match the hash"), "{error}");
    assert!(stored(root).is_empty());
}

#[test]
fn two_live_first_rows_on_one_day_heal_to_the_current_and_on_two_days_keep_a_revision_out() {
    for apart in [false, true] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        run(
            root,
            &one_workout(
                &scratch.path().join("1"),
                "2026-01-05T08:00:00.000-07:00",
                "2026-01-05T08:20:00.000-07:00",
            ),
            "20261104_120000",
            None,
        );
        // A second live copy of its first row, as a stopped move leaves, on its day
        // or the day after.
        let (_, _, first) = placed(root, "w1").remove(0);
        let copy = if apart {
            window(root, "20260106", "090000_300")
        } else {
            window(root, "20260105", "090000_300")
        };
        fs::create_dir_all(&copy).unwrap();
        fs::write(
            copy.join(BODY_RECORDS_FILE),
            solstone_core_format::content::body_records::encode_rows(&[first]),
        )
        .unwrap();
        let rid = oura_row_id("workout", "w1");
        let before = rows_of(root, &rid);
        let bytes = stream_bytes(root);
        let mut revised = workout(
            "w1",
            "running",
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:20:00.000-07:00",
        );
        revised["calories"] = json!(500.0);
        let outcome = run(
            root,
            &stage(&scratch.path().join("2"), &[("workout", vec![revised])]),
            "20261104_120001",
            None,
        );
        if apart {
            assert_eq!(stream_bytes(root), bytes);
            assert!(outcome.summary.contains("Oura has since changed 1 entry"));
        } else {
            let windows = windows_from("20260105", (8, 0), 300, 4);
            assert_converged(root, &rid, &before, &tiling(&rid, &as_refs(&windows)));
            assert_eq!(placed(root, "w1")[0].2.values["energy"], "500.0 kcal");
            assert!(outcome.holes.is_empty(), "{:?}", outcome.holes);
            // Healing a copy is no change Oura made to its time.
            assert!(
                !outcome.summary.contains("has since changed"),
                "{}",
                outcome.summary
            );
        }
    }
}
#[test]
fn the_lookup_reaches_one_day_either_side_and_no_further() {
    // A workout Oura moves a day later, by its own date and its start, is found
    // where it was and kept as first written.
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    let first = one_workout(
        &scratch.path().join("1"),
        "2026-01-05T08:00:00.000-07:00",
        "2026-01-05T08:20:00.000-07:00",
    );
    run(root, &first, "20261104_120000", None);
    let near = run(
        root,
        &one_workout(
            &scratch.path().join("2"),
            "2026-01-06T08:00:00.000-07:00",
            "2026-01-06T08:20:00.000-07:00",
        ),
        "20261104_120001",
        None,
    );
    assert!(near.summary.contains("Oura has since changed 1 entry"));
    assert_eq!(placed(root, "w1").len(), 1);

    // Two days later is past the lookup's reach: it lands as new, beside the
    // first, which is the documented limit.
    let far = run(
        root,
        &one_workout(
            &scratch.path().join("3"),
            "2026-01-07T08:00:00.000-07:00",
            "2026-01-07T08:20:00.000-07:00",
        ),
        "20261104_120002",
        None,
    );
    assert!(
        !far.summary.contains("has since changed"),
        "{}",
        far.summary
    );
    assert_eq!(placed(root, "w1").len(), 2);

    // A night moved past midnight is found on the day before.
    let journal = journal_with_nothing();
    let root = journal.path();
    let pages = |name: &str, start: &str, end: &str| {
        stage(
            &scratch.path().join(name),
            &[("sleep", vec![sleep("n1", "long_sleep", start, end)])],
        )
    };
    run(
        root,
        &pages(
            "4",
            "2026-01-05T23:40:00.000-07:00",
            "2026-01-06T06:00:00.000-07:00",
        ),
        "20261104_120003",
        None,
    );
    let moved = run(
        root,
        &pages(
            "5",
            "2026-01-06T00:20:00.000-07:00",
            "2026-01-06T06:00:00.000-07:00",
        ),
        "20261104_120004",
        None,
    );
    assert!(moved.summary.contains("Oura has since changed 1 entry"));
    assert_eq!(placed(root, "n1").len(), 1);
    assert_eq!(placed(root, "n1")[0].0, "20260105");
}

#[path = "body_segments_fetch_tests.rs"]
mod fetch;

#[test]
fn a_plain_file_among_the_facets_stops_no_oura_run_and_its_records_are_written() {
    let journal = journal();
    let root = journal.path();
    // A facet tool keeps its candidates in a file beside the facet folders.
    fs::write(root.join("facets/review-candidates.jsonl"), "{}\n").unwrap();
    let first = run(root, &fixture(), "20261104_120000", Some("health"));
    assert!(first.published > 10, "{}", first.summary);
    assert!(
        root.join("facets/health/activities/20260102.jsonl")
            .is_file(),
        "{}",
        first.summary
    );
    // And a revision, which looks up records on its days, runs through as well.
    let again = run(root, &fixture(), "20261104_120001", Some("health"));
    assert_eq!(again.published, 0, "{}", again.summary);
}

#[test]
fn oura_nights_that_overlap_each_other_are_counted_and_nights_apart_are_not() {
    let journal = journal();
    let root = journal.path();
    let scratch = tempfile::tempdir().unwrap();
    // One night Oura gave twice, under two ids, and a night on its own.
    let pages = stage(
        scratch.path(),
        &[(
            "sleep",
            vec![
                sleep(
                    "n1",
                    "long_sleep",
                    "2026-01-05T22:40:00-07:00",
                    "2026-01-06T06:30:00-07:00",
                ),
                sleep(
                    "n2",
                    "long_sleep",
                    "2026-01-05T23:10:00-07:00",
                    "2026-01-06T06:40:00-07:00",
                ),
                sleep(
                    "n3",
                    "long_sleep",
                    "2026-01-06T22:50:00-07:00",
                    "2026-01-07T06:20:00-07:00",
                ),
            ],
        )],
    );
    let outcome = run(root, &pages, "20261104_120000", Some("health"));
    assert!(
        outcome
            .summary
            .contains("2 of Oura's nights in this import overlap another."),
        "{}",
        outcome.summary
    );
    // Twin: nights apart say nothing.
    let journal = self::journal();
    let root = journal.path();
    let apart = stage(
        &scratch.path().join("apart"),
        &[(
            "sleep",
            vec![
                sleep(
                    "n1",
                    "long_sleep",
                    "2026-01-05T22:40:00-07:00",
                    "2026-01-06T06:30:00-07:00",
                ),
                sleep(
                    "n3",
                    "long_sleep",
                    "2026-01-06T22:50:00-07:00",
                    "2026-01-07T06:20:00-07:00",
                ),
            ],
        )],
    );
    let outcome = run(root, &apart, "20261104_120000", Some("health"));
    assert!(
        !outcome.summary.contains("overlap another"),
        "{}",
        outcome.summary
    );
}

#[test]
fn a_move_under_an_import_naming_no_facet_keeps_the_record_readable_under_its_facet() {
    for crowded in [false, true] {
        let journal = journal();
        let root = journal.path();
        let scratch = tempfile::tempdir().unwrap();
        let first = one_workout(
            &scratch.path().join("1"),
            "2026-01-05T08:00:00.000-07:00",
            "2026-01-05T08:20:00.000-07:00",
        );
        run(root, &first, "20261104_120000", Some("health"));
        if crowded {
            // The window the workout moves into already carries another facet.
            let other = stage(
                &scratch.path().join("other"),
                &[(
                    "session",
                    vec![session(
                        "s1",
                        "meditation",
                        "2026-01-05T09:00:00.000-07:00",
                        "2026-01-05T09:04:00.000-07:00",
                    )],
                )],
            );
            run(root, &other, "20261104_120001", Some("other"));
        }
        // Oura moves it an hour later on its day; the owner imports with no facet.
        let moved = one_workout(
            &scratch.path().join("2"),
            "2026-01-05T09:00:00.000-07:00",
            "2026-01-05T09:20:00.000-07:00",
        );
        let outcome = run(root, &moved, "20261104_120002", None);
        let experience = placed(root, "w1")
            .into_iter()
            .find(|(.., row)| row.kind == RowKind::Experience)
            .unwrap();
        assert_eq!(experience.1, "090000_300");
        // A connection limited to the health facet reads windows filed under it.
        let health_only = |day: &str, stream: &str, name: Option<&str>| {
            name.is_some_and(|name| {
                solstone_core_indexer_store::classification::read_segment_facet_assignments(
                    root,
                    &format!("{day}/{stream}/{name}/{BODY_RECORDS_FILE}"),
                )
                .iter()
                .any(|facet| facet == "health")
            })
        };
        let faces = solstone_core_location::body_records::body_faces_range(
            root,
            "health",
            "20260105",
            "20260105",
            Some(&health_only),
            true,
        )
        .unwrap();
        let shown = faces.get("20260105").map_or(0, Vec::len);
        assert_eq!(shown, usize::from(!crowded), "crowded: {crowded}");
        let facet_of = |key: &str| {
            fs::read_to_string(window(root, "20260105", key).join("talents/facets.json")).ok()
        };
        let health = Some(facet_file_text("health"));
        if crowded {
            assert_eq!(facet_of("090000_300"), Some(facet_file_text("other")));
            assert!(
                outcome.summary.contains("1 window already carries a facet"),
                "{}",
                outcome.summary
            );
        } else {
            // Every window of its new tiling, its first and its tiles.
            for key in ["090000_300", "090500_300", "091000_300", "091500_300"] {
                assert_eq!(facet_of(key), health, "{key}");
            }
            assert!(
                outcome
                    .summary
                    .contains("4 windows each took the facet of the entry Oura moved into it, so what's in them is readable under that facet."),
                "{}",
                outcome.summary
            );
        }
    }
}

#[test]
fn nothing_oura_scores_or_judges_reaches_the_days_document_or_search() {
    use solstone_core_format::content::{Family, produce_chunks};
    let journal = journal();
    let root = journal.path();
    run(root, &fixture(), "20261104_120000", Some("health"));
    solstone_core_location::body_day::BodyDays::default().reconcile(root);
    // Every text the journal makes of the import: each day's document and each
    // window's search chunks.
    let mut texts: Vec<(String, String)> = Vec::new();
    for day in fs::read_dir(root.join("chronicle")).unwrap().flatten() {
        let name = day.file_name().to_string_lossy().into_owned();
        if let Ok(text) = fs::read_to_string(day.path().join("body.md")) {
            texts.push((format!("{name}/body.md"), text));
        }
        for window in fs::read_dir(day.path().join(STREAM)).unwrap().flatten() {
            let Ok(text) = fs::read_to_string(window.path().join(BODY_RECORDS_FILE)) else {
                continue;
            };
            let rel = format!(
                "{name}/{STREAM}/{}/{BODY_RECORDS_FILE}",
                window.file_name().to_string_lossy()
            );
            let produced = produce_chunks(Family::BodyRecords, &rel, &text);
            assert!(produced.error.is_none(), "{rel}: {:?}", produced.error);
            for chunk in produced.chunks.into_iter().map(|chunk| chunk.content) {
                texts.push((rel.clone(), chunk));
            }
            if let Some(header) = produced.header {
                texts.push((rel.clone(), header));
            }
        }
    }
    // The instrument reads what it should: the walk, its distance and the day's
    // documents by name.
    let all: String = texts.iter().map(|(_, text)| text.as_str()).collect();
    for expected in ["walking", "2412.9 m", "daily readiness"] {
        assert!(all.contains(expected), "{expected}: {all}");
    }
    // The fixture's scores, levels and measures, and the words Oura judges with.
    let judged = [
        "82",
        "85",
        "74",
        "93",
        "38",
        "44",
        "96.4",
        "41.2",
        "5400",
        "112",
        "stressful",
        "limited",
        "good",
        "moderate",
        "hard",
    ];
    for (place, text) in &texts {
        let words: Vec<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '.'))
            .map(|word| word.trim_matches('.'))
            .collect();
        for word in judged {
            assert!(!words.contains(&word), "{word} in {place}: {text}");
        }
    }
}
