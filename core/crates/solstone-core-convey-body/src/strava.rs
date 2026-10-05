// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strava workouts on a body day.
//!
//! A Strava workout lives in the journal as consecutive pieces of about five
//! minutes under `chronicle/<day>/import.strava/`, each holding the workout's own
//! values in `workout.json`. A day lists each workout that has a piece on it,
//! once, with the values Strava recorded. Nothing is computed from them.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::DateTime;
use serde_json::{Value, json};

use crate::day::{clock, grouped_decimal, number};

const STREAM: &str = "import.strava";

/// The day's Strava workouts, in start order. A piece that can't be read is
/// skipped: this is a view, and the delete re-reads before it removes anything.
pub(crate) fn workouts_on(journal: &Path, day: &str) -> Vec<Value> {
    let dir = journal.join("chronicle").join(day).join(STREAM);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut by_workout: BTreeMap<u64, Value> = BTreeMap::new();
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
        if let Some(workout) = tile.get("workout") {
            by_workout
                .entry(activity_id)
                .or_insert_with(|| workout.clone());
        }
    }
    let mut items = by_workout
        .into_iter()
        .map(|(activity_id, workout)| item(activity_id, &workout, day))
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a["sort"].as_str().cmp(&b["sort"].as_str()));
    for item in &mut items {
        if let Some(map) = item.as_object_mut() {
            map.remove("sort");
        }
    }
    items
}

fn item(activity_id: u64, workout: &Value, day: &str) -> Value {
    let field = |key: &str| workout.get(key).and_then(Value::as_f64);
    let name = workout
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| workout.get("type").and_then(Value::as_str))
        .unwrap_or("workout")
        .to_owned();
    let start = workout
        .get("start")
        .and_then(Value::as_str)
        .and_then(|start| DateTime::parse_from_rfc3339(start).ok());
    let start_label = start.map(|start| {
        let local = start.naive_local();
        let label = clock(local);
        if local.format("%Y%m%d").to_string() == day {
            label
        } else {
            format!(
                "{}, {label}",
                local.format("%b %-d").to_string().to_lowercase()
            )
        }
    });
    let duration = field("elapsed_seconds").map(duration_label);
    let mut details = Vec::new();
    if let Some(moving) = field("moving_seconds") {
        details.push(format!("moving {}", duration_label(moving)));
    }
    if let Some(metres) = field("distance_m") {
        details.push(format!("{} km", grouped_decimal(metres / 1000.0, 1)));
    }
    if let Some(gain) = field("elevation_gain_m") {
        details.push(format!("elevation gain {} m", number(gain)));
    }
    match (field("heart_rate_avg_bpm"), field("heart_rate_max_bpm")) {
        (Some(avg), Some(max)) => details.push(format!(
            "heart rate avg {}, max {} bpm",
            number(avg),
            number(max)
        )),
        (Some(avg), None) => details.push(format!("heart rate avg {} bpm", number(avg))),
        (None, Some(max)) => details.push(format!("heart rate max {} bpm", number(max))),
        (None, None) => {}
    }
    match (field("power_avg_w"), field("power_weighted_w")) {
        (Some(avg), Some(weighted)) => details.push(format!(
            "power avg {} W, weighted {} W",
            number(avg),
            number(weighted)
        )),
        (Some(avg), None) => details.push(format!("power avg {} W", number(avg))),
        (None, Some(weighted)) => details.push(format!("power weighted {} W", number(weighted))),
        (None, None) => {}
    }
    if let Some(kcal) = field("calories_kcal") {
        details.push(format!("{} kcal", number(kcal)));
    }
    if workout.get("commute").and_then(Value::as_bool) == Some(true) {
        details.push("commute".to_owned());
    }
    if workout.get("entered_by_hand").and_then(Value::as_bool) == Some(true) {
        details.push("entered by hand".to_owned());
    }
    let distance_label =
        field("distance_m").map(|m| format!("{} km", grouped_decimal(m / 1000.0, 1)));
    json!({
        "name": name,
        "source": "Strava",
        "start": start_label,
        "duration": duration,
        "metrics_label": distance_label.unwrap_or_default(),
        "details": details,
        "strava_activity_id": activity_id.to_string(),
        "sort": start.map(|s| s.to_rfc3339()).unwrap_or_default(),
    })
}

fn duration_label(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as i64;
    if minutes < 60 {
        format!("{minutes} min")
    } else if minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{} h {} min", minutes / 60, minutes % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "elapsed_seconds":2880.0,"distance_m":10200.0,"commute":true});
        piece(root.path(), "20261003", "233000_300", 7, ride.clone());
        piece(root.path(), "20261004", "000000_300", 7, ride.clone());
        piece(root.path(), "20261004", "000500_300", 7, ride);
        piece(root.path(), "20261004", "070200_300", 9, run.clone());
        piece(root.path(), "20261004", "070700_300", 9, run);

        let items = workouts_on(root.path(), "20261004");
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
        assert_eq!(workouts_on(root.path(), "20261003").len(), 1);
        assert!(workouts_on(root.path(), "20261005").is_empty());
    }
}
