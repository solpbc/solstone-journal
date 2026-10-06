// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A Strava workout as journal content.
//!
//! A workout is kept as consecutive pieces of about five minutes, each holding
//! the whole workout's values in `workout.json`. Only the first piece renders,
//! so a workout is one search result and one input to thinking, on the day it
//! started. The text restates what Strava recorded, attributed to Strava, and
//! computes nothing from it.

use chrono::DateTime;
use serde_json::Value;

use super::{JsonObject, ProducedChunks, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .first()
        .filter(|tile| is_first_piece(tile))
        .and_then(|tile| {
            let workout = tile.get("workout")?.as_object()?;
            let content = render_workout(workout);
            let occurrence = workout
                .get("start")
                .and_then(Value::as_str)
                .and_then(|start| DateTime::parse_from_rfc3339(start).ok())
                .map(|start| start.timestamp_millis())
                .unwrap_or(0);
            Some(recorded_chunk(content, occurrence, tile))
        })
        .into_iter()
        .collect();
    ProducedChunks {
        chunks,
        agent_override: Some("import.strava".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn is_first_piece(tile: &JsonObject) -> bool {
    tile.get("tile")
        .and_then(|piece| piece.get("index"))
        .and_then(Value::as_u64)
        .is_none_or(|index| index == 0)
}

fn render_workout(workout: &JsonObject) -> String {
    let text = |key: &str| {
        workout
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let number = |key: &str| workout.get(key).and_then(Value::as_f64);
    let kind = text("type");
    let name = text("name").or(kind).unwrap_or("workout");
    let mut lines = vec![format!("## Strava workout: {name}")];

    let mut when = Vec::new();
    if let Some(kind) = kind {
        when.push(kind.to_string());
    }
    if let Some(start) = text("start").and_then(|start| DateTime::parse_from_rfc3339(start).ok()) {
        when.push(format!(
            "started {}",
            start.naive_local().format("%Y-%m-%d %H:%M")
        ));
    }
    if let Some(seconds) = number("elapsed_seconds") {
        when.push(format!("{} elapsed", duration(seconds)));
    }
    if let Some(seconds) = number("moving_seconds") {
        when.push(format!("{} moving", duration(seconds)));
    }
    push_line(&mut lines, when);

    let mut distance = Vec::new();
    if let Some(metres) = number("distance_m") {
        distance.push(format!("{:.1} km", metres / 1000.0));
    }
    if let Some(gain) = number("elevation_gain_m") {
        distance.push(format!("elevation gain {} m", whole(gain)));
    }
    push_line(&mut lines, distance);

    let mut recorded = Vec::new();
    match (number("heart_rate_avg_bpm"), number("heart_rate_max_bpm")) {
        (Some(avg), Some(max)) => recorded.push(format!(
            "heart rate average {} bpm, max {} bpm",
            whole(avg),
            whole(max)
        )),
        (Some(avg), None) => recorded.push(format!("heart rate average {} bpm", whole(avg))),
        (None, Some(max)) => recorded.push(format!("heart rate max {} bpm", whole(max))),
        (None, None) => {}
    }
    match (number("power_avg_w"), number("power_weighted_w")) {
        (Some(avg), Some(weighted)) => recorded.push(format!(
            "power average {} W, weighted {} W",
            whole(avg),
            whole(weighted)
        )),
        (Some(avg), None) => recorded.push(format!("power average {} W", whole(avg))),
        (None, Some(weighted)) => recorded.push(format!("power weighted {} W", whole(weighted))),
        (None, None) => {}
    }
    if let Some(kcal) = number("calories_kcal") {
        recorded.push(format!("{} kcal", whole(kcal)));
    }
    if !recorded.is_empty() {
        lines.push(format!("as Strava recorded it: {}", recorded.join(" · ")));
    }

    let mut flags = Vec::new();
    if workout.get("commute").and_then(Value::as_bool) == Some(true) {
        flags.push("commute".to_string());
    }
    if workout.get("entered_by_hand").and_then(Value::as_bool) == Some(true) {
        flags.push("entered by hand".to_string());
    }
    push_line(&mut lines, flags);

    if let Some(fields) = workout.get("strava_fields").and_then(Value::as_object) {
        let fields = fields
            .iter()
            .filter_map(|(label, value)| {
                let value = value.as_str()?.trim();
                (!value.is_empty()).then(|| format!("- {}: {value}", label.to_lowercase()))
            })
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            lines.push(String::new());
            lines.push("more from Strava:".to_string());
            lines.extend(fields);
        }
    }
    lines.join("\n")
}

fn push_line(lines: &mut Vec<String>, parts: Vec<String>) {
    if !parts.is_empty() {
        lines.push(parts.join(" · "));
    }
}

fn whole(value: f64) -> String {
    format!("{}", value.round() as i64)
}

fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as i64;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::content::{ContentResolution, Family, classify, produce_chunks};

    const REL: &str = "20261004/import.strava/070200_300/workout.json";

    fn tile(index: u64) -> Value {
        json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20261004T120000",
            "activity_id": 9,
            "zone": "America/Denver",
            "tile": {"index": index, "count": 10},
            "workout": {
                "name": "Morning Run",
                "type": "Run",
                "start": "2026-10-04T07:02:00-06:00",
                "elapsed_seconds": 2880,
                "moving_seconds": 2700,
                "distance_m": 10200.0,
                "elevation_gain_m": 85.4,
                "heart_rate_avg_bpm": 152.3,
                "heart_rate_max_bpm": 171.0,
                "power_avg_w": null,
                "power_weighted_w": null,
                "calories_kcal": 640.0,
                "commute": false,
                "entered_by_hand": false,
                "strava_fields": {
                    "Activity Description": "easy loop by the creek",
                    "Activity Gear": "Pegasus 41"
                }
            }
        })
    }

    #[test]
    fn a_workout_is_indexed_transcript_content() {
        assert_eq!(classify(REL), ContentResolution::Indexed(Family::Workout));
    }

    #[test]
    fn the_first_piece_restates_what_strava_recorded() {
        let produced = produce_chunks(Family::Workout, REL, &tile(0).to_string());
        assert_eq!(produced.agent_override.as_deref(), Some("import.strava"));
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            produced.chunks[0].content,
            "## Strava workout: Morning Run\n\
             Run · started 2026-10-04 07:02 · 48 min elapsed · 45 min moving\n\
             10.2 km · elevation gain 85 m\n\
             as Strava recorded it: heart rate average 152 bpm, max 171 bpm · 640 kcal\n\
             \n\
             more from Strava:\n\
             - activity description: easy loop by the creek\n\
             - activity gear: Pegasus 41"
        );
        assert_eq!(
            produced.chunks[0].occurrence_time_ms.map(|time| time.0),
            Some(1_791_118_920_000)
        );
    }

    #[test]
    fn later_pieces_of_the_same_workout_render_nothing() {
        let produced = produce_chunks(Family::Workout, REL, &tile(3).to_string());
        assert!(produced.chunks.is_empty());
        assert!(produced.error.is_none());
    }

    #[test]
    fn unreadable_tiles_render_nothing() {
        for text in ["", "not json", "[]", "{\"tile\":{\"index\":0}}"] {
            assert!(produce_chunks(Family::Workout, REL, text).chunks.is_empty());
        }
    }
}
