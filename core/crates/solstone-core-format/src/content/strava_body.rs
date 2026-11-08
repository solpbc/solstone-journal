// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A Strava tile read as body rows.
//!
//! Strava keeps its own tiles: each piece of a workout is a `workout.json`
//! holding the whole workout. Nothing is written in the body-row form. This is
//! how the body readers see a tile: its first piece as the workout's
//! `experience`, and every later piece as an `experience_tile` of it.

use chrono::{DateTime, Duration, SecondsFormat};
use serde_json::Value;

use super::body_records::{BodyRow, RowKind, plain_words};

/// The stream Strava's tiles are in.
pub const STRAVA_STREAM: &str = "import.strava";
/// The one file a Strava tile holds.
pub const STRAVA_TILE_FILE: &str = "workout.json";
/// What a Strava tile's rows name as their source.
pub const STRAVA_SOURCE: &str = "strava";
/// Who recorded a Strava workout, as its rows say it.
pub const STRAVA_BY: &str = "Strava";
const TILE_SCHEMA: &str = "solstone.import.strava.tile.v1";

/// The export headers of the owner's own words on a workout, English and
/// German: its description and its private note.
pub const DESCRIPTION_HEADERS: [&str; 2] = ["Activity Description", "Aktivitätsbeschreibung"];
pub const PRIVATE_NOTE_HEADERS: [&str; 2] = [
    "Activity Private Note",
    "Hinweis zur Privatsphäre für Aktivitäten",
];
/// The export headers of the owner's gear, English and German.
pub const GEAR_HEADERS: [&str; 2] = ["Activity Gear", "Aktivitätsausrüstung"];

/// The first non-empty value under any of `headers` in a workout's
/// `strava_fields`.
#[must_use]
pub fn strava_field<'a>(strava_fields: Option<&'a Value>, headers: &[&str]) -> Option<&'a str> {
    let fields = strava_fields?.as_object()?;
    headers.iter().find_map(|header| {
        fields
            .get(*header)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// One Strava tile's rows: the first piece is the workout's `experience`, a
/// later piece is an `experience_tile` of it. `None` when the text isn't a
/// tile this code can read: not JSON, another schema, or no activity id, type
/// or start.
#[must_use]
pub fn strava_rows(workout_json: &str) -> Option<Vec<BodyRow>> {
    let tile: Value = serde_json::from_str(workout_json).ok()?;
    if tile.get("schema").and_then(Value::as_str) != Some(TILE_SCHEMA) {
        return None;
    }
    let activity_id = tile.get("activity_id")?.as_u64()?.to_string();
    let piece = tile.get("tile")?;
    let index = piece.get("index")?.as_u64()?;
    let count = piece.get("count").and_then(Value::as_u64);
    let workout = tile.get("workout")?.as_object()?;
    let record_type = workout.get("type")?.as_str()?.trim();
    if record_type.is_empty() {
        return None;
    }
    let start = DateTime::parse_from_rfc3339(workout.get("start")?.as_str()?.trim()).ok()?;
    // The span its tiles cover, as they were cut: at least a second.
    let elapsed = workout
        .get("elapsed_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(1);
    let end = start + Duration::seconds(i64::try_from(elapsed).ok()?);
    let label = plain_words(record_type);
    let kind = if index == 0 {
        RowKind::Experience
    } else {
        RowKind::ExperienceTile
    };
    let id = if index == 0 {
        activity_id.clone()
    } else {
        format!("{activity_id}-{index}")
    };
    let mut row = BodyRow::new(kind, id, STRAVA_SOURCE, record_type, label);
    row.by = Some(STRAVA_BY.to_owned());
    row.experience = Some("workout".to_owned());
    row.start = Some(start.to_rfc3339_opts(SecondsFormat::Secs, false));
    row.end = Some(end.to_rfc3339_opts(SecondsFormat::Secs, false));
    if index > 0 {
        row.of = Some(activity_id);
        row.index = Some(index);
        row.count = count;
        return Some(vec![row]);
    }
    if let Some(distance) = workout.get("distance_m").filter(|value| value.is_number()) {
        row.values
            .insert("distance".to_owned(), format!("{distance} m"));
    }
    let name = workout
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    row.words = name
        .into_iter()
        .chain(strava_field(
            workout.get("strava_fields"),
            &DESCRIPTION_HEADERS,
        ))
        .chain(strava_field(
            workout.get("strava_fields"),
            &PRIVATE_NOTE_HEADERS,
        ))
        .map(str::to_owned)
        .collect();
    row.fields = workout
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(vec![row])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::content::body_records::{decode_rows, encode_rows};

    fn tile(index: u64, strava_fields: Option<Value>) -> Value {
        let mut workout = json!({
            "name": "Hill repeats with Sam",
            "type": "Run",
            "start": "2026-10-04T07:02:00-06:00",
            "elapsed_seconds": 2880,
            "moving_seconds": 2700,
            "distance_m": 10200.0,
            "elevation_gain_m": 85.4,
            "heart_rate_avg_bpm": null,
            "heart_rate_max_bpm": null,
            "power_avg_w": null,
            "power_weighted_w": null,
            "calories_kcal": 640.0,
            "commute": false,
            "entered_by_hand": false,
        });
        if let Some(fields) = strava_fields {
            workout["strava_fields"] = fields;
        }
        json!({
            "schema": TILE_SCHEMA,
            "import_id": "20261004_120000",
            "activity_id": 9_000_001_u64,
            "zone": "America/Denver",
            "tile": {"index": index, "count": 10, "start": "2026-10-04T07:02:00-06:00",
                "end": "2026-10-04T07:07:00-06:00", "seconds": 300},
            "workout": workout,
        })
    }

    #[test]
    fn a_first_piece_is_the_whole_workout() {
        let text = tile(
            0,
            Some(json!({
                "Activity Description": "easy loop by the creek",
                "Activity Private Note": "left knee felt tight",
                "Activity Gear": "Trek Domane SL6 (Blue Thunder)",
                "Bike": "b123",
            })),
        );
        let rows = strava_rows(&text.to_string()).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, RowKind::Experience);
        assert_eq!(row.id, "9000001");
        assert_eq!(row.source, "strava");
        assert_eq!(row.by.as_deref(), Some("Strava"));
        assert_eq!(row.experience.as_deref(), Some("workout"));
        assert_eq!(row.record_type, "Run");
        assert_eq!(row.start.as_deref(), Some("2026-10-04T07:02:00-06:00"));
        // 2880 seconds later, at the same offset.
        assert_eq!(row.end.as_deref(), Some("2026-10-04T07:50:00-06:00"));
        assert_eq!(
            row.values.get("distance").map(String::as_str),
            Some("10200.0 m")
        );
        assert_eq!(
            row.words,
            [
                "Hill repeats with Sam",
                "easy loop by the creek",
                "left knee felt tight"
            ]
        );
        assert_eq!(
            Value::Object(row.fields.clone().into_iter().collect()),
            text["workout"]
        );
        assert!(row.of.is_none() && row.index.is_none());
        // It round-trips as a stored row would.
        assert_eq!(decode_rows(&encode_rows(&rows)).unwrap(), rows);
    }

    #[test]
    fn a_later_piece_is_a_tile_of_the_workout_with_its_whole_span_and_nothing_else() {
        let rows = strava_rows(&tile(3, None).to_string()).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, RowKind::ExperienceTile);
        assert_eq!(row.of.as_deref(), Some("9000001"));
        assert_eq!(row.index, Some(3));
        assert_eq!(row.count, Some(10));
        assert_ne!(row.id, "9000001");
        assert_eq!(row.start.as_deref(), Some("2026-10-04T07:02:00-06:00"));
        assert_eq!(row.end.as_deref(), Some("2026-10-04T07:50:00-06:00"));
        assert_eq!(row.by.as_deref(), Some("Strava"));
        assert!(!row.label.is_empty());
        assert!(row.words.is_empty() && row.values.is_empty() && row.fields.is_empty());
        assert_eq!(decode_rows(&encode_rows(&rows)).unwrap(), rows);
    }

    /// A tile written before every other column was kept has no
    /// `strava_fields`, and reads the same, with only its name for words.
    #[test]
    fn a_tile_with_no_strava_fields_reads_with_its_name_alone() {
        let text = tile(0, None);
        let rows = strava_rows(&text.to_string()).unwrap();
        assert_eq!(rows[0].words, ["Hill repeats with Sam"]);
        assert!(!rows[0].fields.contains_key("strava_fields"));
        assert_eq!(decode_rows(&encode_rows(&rows)).unwrap(), rows);
        // German headers are read the same.
        let german = tile(
            0,
            Some(json!({
                "Aktivitätsbeschreibung": "Runde am See",
                "Hinweis zur Privatsphäre für Aktivitäten": "Knie zwickt",
            })),
        );
        assert_eq!(
            strava_rows(&german.to_string()).unwrap()[0].words,
            ["Hill repeats with Sam", "Runde am See", "Knie zwickt"]
        );
    }

    #[test]
    fn no_distance_no_value_and_an_empty_name_no_word() {
        let mut text = tile(0, None);
        text["workout"]["distance_m"] = Value::Null;
        text["workout"]["name"] = json!("  ");
        let rows = strava_rows(&text.to_string()).unwrap();
        assert!(rows[0].values.is_empty());
        assert!(rows[0].words.is_empty());
    }

    /// A first piece with `name` and `type`, and the owner's other columns
    /// under one language's headers.
    fn named(name: &str, record_type: &str, german: bool, extra: &[(usize, &str)]) -> BodyRow {
        // The export's own headers, as Strava writes them in each language.
        let headers = if german {
            [
                "Aktivitätsbeschreibung",
                "Hinweis zur Privatsphäre für Aktivitäten",
                "Aktivitätsausrüstung",
            ]
        } else {
            [
                "Activity Description",
                "Activity Private Note",
                "Activity Gear",
            ]
        };
        let fields = extra
            .iter()
            .map(|(column, value)| (headers[*column].to_owned(), json!(value)))
            .collect::<serde_json::Map<_, _>>();
        let mut text = tile(0, (!fields.is_empty()).then_some(Value::Object(fields)));
        text["workout"]["name"] = json!(name);
        text["workout"]["type"] = json!(record_type);
        strava_rows(&text.to_string()).unwrap().remove(0)
    }

    const DESCRIPTION: usize = 0;
    const NOTE: usize = 1;
    const GEAR: usize = 2;

    #[test]
    fn a_workouts_own_words_are_its_owners_name_description_and_note_in_either_language() {
        use crate::content::body_records::owner_words;
        for german in [false, true] {
            // Names Strava writes itself, in English and German.
            for (name, record_type) in [
                ("", "Run"),
                ("Morning Run", "Run"),
                ("Lunch Ride", "Ride"),
                ("Lauf am Morgen", "Lauf"),
                ("Fahrt am Nachmittag", "Fahrt"),
                ("Radfahrt am Abend", "Radfahrt"),
                ("LAUF AM MORGEN", "Lauf"),
                ("Lauf in der Nacht", "Lauf"),
            ] {
                let row = named(name, record_type, german, &[]);
                assert!(owner_words("strava", &row).is_empty(), "{name}");
            }
            // The owner's names, and an app's.
            for (name, record_type) in [
                ("Hill repeats with Sam", "Run"),
                ("Sonntagsrunde mit Anna", "Radfahrt"),
                ("Zwift - Watopia", "VirtualRide"),
                ("Lauf am Morgen mit Anna", "Lauf"),
            ] {
                let row = named(name, record_type, german, &[]);
                assert_eq!(owner_words("strava", &row), [name], "{name}");
            }
            // A description an app appended to, and a private note, beside a
            // default name: both kept, the name not.
            let weather = "Hill loop.\nCloudy, 12°C, feels like 10°C, wind 9 km/h";
            let row = named(
                "Morning Run",
                "Run",
                german,
                &[
                    (DESCRIPTION, weather),
                    (NOTE, "left knee felt tight"),
                    (GEAR, "Trek Domane SL6 (Blue Thunder)"),
                ],
            );
            assert_eq!(
                owner_words("strava", &row),
                [weather, "left knee felt tight"],
                "german: {german}"
            );
        }
    }

    #[test]
    fn a_workouts_gear_is_its_own_and_no_other_sources() {
        use crate::content::body_records::{owner_gear, owner_words};
        for german in [false, true] {
            let row = named(
                "Hill repeats with Sam",
                "Ride",
                german,
                &[(GEAR, "Trek Domane SL6 (Blue Thunder)")],
            );
            assert_eq!(
                owner_gear("strava", &row),
                ["Trek Domane SL6 (Blue Thunder)"],
                "german: {german}"
            );
            // Gear isn't among the words.
            assert_eq!(owner_words("strava", &row), ["Hill repeats with Sam"]);
            // The same row under another source names no gear.
            assert!(owner_gear("oura", &row).is_empty());
            assert!(owner_gear("apple_health", &row).is_empty());
        }
        // No gear, or a tile with no other columns at all.
        assert!(owner_gear("strava", &named("Run", "Run", false, &[(NOTE, "x")])).is_empty());
        assert!(owner_gear("strava", &named("Run", "Run", false, &[])).is_empty());
    }

    /// The German defaults are Strava's alone: an Oura tag's note with the same
    /// shape is the owner's, as it was.
    #[test]
    fn german_defaults_are_only_stravas() {
        use crate::content::body_records::owner_words;
        let mut tag = BodyRow::new(
            RowKind::Logged,
            "o1".to_owned(),
            "oura",
            "custom",
            "custom".to_owned(),
        );
        tag.words = vec![
            "Kaffee am Morgen".to_owned(),
            "Sauna in der Nacht".to_owned(),
        ];
        assert_eq!(
            owner_words("oura", &tag),
            ["Kaffee am Morgen", "Sauna in der Nacht"]
        );
    }

    #[test]
    fn what_isnt_a_tile_reads_as_nothing() {
        assert!(strava_rows("{not json").is_none());
        let mut other = tile(0, None);
        other["schema"] = json!("something.else");
        assert!(strava_rows(&other.to_string()).is_none());
        let mut no_start = tile(0, None);
        no_start["workout"]["start"] = json!("");
        assert!(strava_rows(&no_start.to_string()).is_none());
        let mut no_id = tile(0, None);
        no_id
            .as_object_mut()
            .unwrap()
            .remove("activity_id")
            .unwrap();
        assert!(strava_rows(&no_id.to_string()).is_none());
    }
}
