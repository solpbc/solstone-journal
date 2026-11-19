// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strava workouts on a body day.
//!
//! A Strava workout lives in the journal as consecutive pieces of about five
//! minutes under `chronicle/<day>/import.strava/`, each holding the workout's own
//! values in `workout.json`. A day lists each workout that has a piece on it,
//! once, with the values Strava recorded. Nothing is computed from them. A
//! workout goes with the day it started: when the owner hid that day, the
//! days after it leave it out too.

use solstone_core_convey_http::window_catalog::{strings, text};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_format::content::body_hidden_days::day_of;

use crate::fmt::{clock, grouped_decimal, number, short_day};

const STREAM: &str = "import.strava";

/// The day's Strava workouts, in start order, and the hidden days whose
/// workouts it left out. A piece that can't be read is skipped: this is a view,
/// and the delete re-reads before it removes anything.
pub(crate) fn workouts_on(
    journal: &Path,
    day: &str,
    hidden: &BTreeSet<String>,
    locale: &str,
) -> (Vec<Value>, BTreeSet<String>) {
    let mut took = BTreeSet::new();
    let dir = journal.join("chronicle").join(day).join(STREAM);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), took);
    };
    let mut by_workout: BTreeMap<u64, Value> = BTreeMap::new();
    let mut starts_here = BTreeSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Some(tile) = std::fs::read(entry.path().join("workout.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let Some(activity_id) = tile.get("activity_id").and_then(Value::as_u64) else {
            continue;
        };
        if tile.pointer("/tile/index").and_then(Value::as_u64) == Some(0) {
            starts_here.insert(activity_id);
        }
        if let Some(workout) = tile.get("workout") {
            by_workout
                .entry(activity_id)
                .or_insert_with(|| workout.clone());
        }
    }
    if !hidden.is_empty() {
        let zone = solstone_core_journal_config::placed_zone(journal, "strava");
        by_workout.retain(|activity_id, workout| {
            if starts_here.contains(activity_id) {
                return true;
            }
            let started = workout
                .get("start")
                .and_then(Value::as_str)
                .and_then(|start| day_of(start, zone.as_ref()));
            match started {
                Some(started) if hidden.contains(&started) => {
                    took.insert(started);
                    false
                }
                _ => true,
            }
        });
    }
    let mut items = by_workout
        .into_iter()
        .map(|(activity_id, workout)| item(activity_id, &workout, day, locale))
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a["sort"].as_str().cmp(&b["sort"].as_str()));
    for item in &mut items {
        if let Some(map) = item.as_object_mut() {
            map.remove("sort");
        }
    }
    (items, took)
}

fn item(activity_id: u64, workout: &Value, day: &str, locale: &str) -> Value {
    let field = |key: &str| workout.get(key).and_then(Value::as_f64);
    let name = workout
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| workout.get("type").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| text(locale, "journal.window.body.strava.workout"));
    let start = workout
        .get("start")
        .and_then(Value::as_str)
        .and_then(|start| solstone_core_time::parse_rfc3339(start).ok());
    let start_label = start.as_ref().map(|start| {
        let local = start.datetime();
        let label = clock(local, locale);
        if solstone_core_time::day_key(local.date()) == day {
            label
        } else {
            strings(
                locale,
                "journal.window.body.date.time.before",
                [
                    (
                        "date",
                        &short_day(&solstone_core_time::day_key(local.date()), locale)
                            .unwrap_or_default()
                            .to_lowercase(),
                    ),
                    ("time", &label),
                ],
            )
        }
    });
    let duration = field("elapsed_seconds").map(|seconds| duration_label(seconds, locale));
    let mut details = Vec::new();
    if let Some(moving) = field("moving_seconds") {
        details.push(strings(
            locale,
            "journal.window.body.strava.moving",
            [("duration", &duration_label(moving, locale))],
        ));
    }
    if let Some(metres) = field("distance_m") {
        details.push(strings(
            locale,
            "journal.window.body.strava.distance",
            [("distance", &grouped_decimal(metres / 1000.0, 1))],
        ));
    }
    if let Some(gain) = field("elevation_gain_m") {
        details.push(strings(
            locale,
            "journal.window.body.strava.elevation",
            [("metres", &number(gain))],
        ));
    }
    match (field("heart_rate_avg_bpm"), field("heart_rate_max_bpm")) {
        (Some(avg), Some(max)) => details.push(strings(
            locale,
            "journal.window.body.strava.heart",
            [("average", &number(avg)), ("maximum", &number(max))],
        )),
        (Some(avg), None) => details.push(strings(
            locale,
            "journal.window.body.strava.heart.average",
            [("value", &number(avg))],
        )),
        (None, Some(max)) => details.push(strings(
            locale,
            "journal.window.body.strava.heart.maximum",
            [("value", &number(max))],
        )),
        (None, None) => {}
    }
    match (field("power_avg_w"), field("power_weighted_w")) {
        (Some(avg), Some(weighted)) => details.push(strings(
            locale,
            "journal.window.body.strava.power",
            [("average", &number(avg)), ("weighted", &number(weighted))],
        )),
        (Some(avg), None) => details.push(strings(
            locale,
            "journal.window.body.strava.power.average",
            [("value", &number(avg))],
        )),
        (None, Some(weighted)) => details.push(strings(
            locale,
            "journal.window.body.strava.power.weighted",
            [("value", &number(weighted))],
        )),
        (None, None) => {}
    }
    if let Some(kcal) = field("calories_kcal") {
        details.push(strings(
            locale,
            "journal.window.body.strava.energy",
            [("value", &number(kcal))],
        ));
    }
    if workout.get("commute").and_then(Value::as_bool) == Some(true) {
        details.push(text(locale, "journal.window.body.strava.commute"));
    }
    if workout.get("entered_by_hand").and_then(Value::as_bool) == Some(true) {
        details.push(text(locale, "journal.window.body.strava.manual"));
    }
    let distance_label = field("distance_m").map(|m| {
        strings(
            locale,
            "journal.window.body.strava.distance",
            [("distance", &grouped_decimal(m / 1000.0, 1))],
        )
    });
    // Every other column Strava exported for the workout, as Strava labelled it
    // and with its value as written.
    let more_from_strava = workout
        .get("strava_fields")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|(label, value)| {
                    let value = value.as_str()?.trim();
                    (!value.is_empty()).then(|| json!({"label": label, "value": value}))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": name,
        "source": "Strava",
        "start": start_label,
        "duration": duration,
        "metrics_label": distance_label.unwrap_or_default(),
        "details": details,
        "more_from_strava": more_from_strava,
        "strava_activity_id": activity_id.to_string(),
        "sort": start
            .map(|s| solstone_core_time::zoned_rfc3339(&s, solstone_core_time::Precision::AutoSi, false))
            .unwrap_or_default(),
    })
}

fn duration_label(seconds: f64, locale: &str) -> String {
    let minutes = (seconds / 60.0).round() as i64;
    if minutes < 60 {
        strings(
            locale,
            "journal.window.body.strava.duration.minutes",
            [("minutes", &minutes.to_string())],
        )
    } else if minutes % 60 == 0 {
        strings(
            locale,
            "journal.window.body.strava.duration.hours",
            [("hours", &(minutes / 60).to_string())],
        )
    } else {
        strings(
            locale,
            "journal.window.body.strava.duration.time",
            [
                ("hours", &(minutes / 60).to_string()),
                ("minutes", &(minutes % 60).to_string()),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_convey_http::window_catalog::ENGLISH;

    fn piece(root: &Path, day: &str, key: &str, activity: u64, workout: Value) {
        let dir = root.join("chronicle").join(day).join(STREAM).join(key);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("workout.json"),
            json!({"activity_id": activity, "import_id": "i", "workout": workout}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_day_lists_each_workout_with_a_piece_on_it_once_with_its_own_values() {
        let root = tempfile::tempdir().unwrap();
        let ride = json!({"name":"Night Ride","type":"Ride","start":"2026-10-03T23:30:00-06:00",
            "elapsed_seconds":7800.0,"moving_seconds":7200.0,"distance_m":60000.0,
            "heart_rate_avg_bpm":140.0,"heart_rate_max_bpm":171.0,"calories_kcal":900.0,
            "commute":false,"entered_by_hand":false});
        let run = json!({"name":"Morning Run","type":"Run","start":"2026-10-04T07:02:00-06:00",
            "elapsed_seconds":2880.0,"distance_m":10200.0,"commute":true,
            "strava_fields":{"Activity Gear":"Pegasus 41","Relative Effort":"38","Weather":""}});
        piece(root.path(), "20261003", "233000_300", 7, ride.clone());
        piece(root.path(), "20261004", "000000_300", 7, ride.clone());
        piece(root.path(), "20261004", "000500_300", 7, ride);
        piece(root.path(), "20261004", "070200_300", 9, run.clone());
        piece(root.path(), "20261004", "070700_300", 9, run);

        let none = BTreeSet::new();
        let (items, took) = workouts_on(root.path(), "20261004", &none, ENGLISH);
        assert!(took.is_empty());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "Night Ride");
        assert_eq!(items[0]["start"], "oct 3, 11:30 PM");
        assert_eq!(items[0]["duration"], "2 h 10 min");
        assert_eq!(items[0]["metrics_label"], "60.0 km");
        assert_eq!(items[0]["strava_activity_id"], "7");
        assert_eq!(items[1]["name"], "Morning Run");
        assert_eq!(items[1]["start"], "7:02 AM");
        assert_eq!(items[1]["duration"], "48 min");
        assert!(
            items[1]["details"]
                .as_array()
                .unwrap()
                .contains(&json!("commute"))
        );
        assert_eq!(
            items[1]["more_from_strava"],
            json!([
                {"label": "Activity Gear", "value": "Pegasus 41"},
                {"label": "Relative Effort", "value": "38"}
            ])
        );
        assert_eq!(items[0]["more_from_strava"], json!([]));
        assert_eq!(
            workouts_on(root.path(), "20261003", &none, ENGLISH).0.len(),
            1,
        );
        assert!(
            workouts_on(root.path(), "20261005", &none, ENGLISH)
                .0
                .is_empty()
        );

        // The night ride started on a day the owner hid: the next day leaves it
        // out and says which day took it, and keeps its own morning run.
        let hidden = BTreeSet::from(["20261003".to_owned()]);
        let (items, took) = workouts_on(root.path(), "20261004", &hidden, ENGLISH);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "Morning Run");
        assert_eq!(took, hidden);
        // Hiding the next day instead leaves the ride on the day it started.
        let next = BTreeSet::from(["20261004".to_owned()]);
        let (items, took) = workouts_on(root.path(), "20261003", &next, ENGLISH);
        assert_eq!(items.len(), 1);
        assert!(took.is_empty());
    }
}
