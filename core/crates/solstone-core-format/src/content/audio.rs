// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, capitalize, display_value, recorded_chunk, truthy_display,
};
use crate::segment::{is_date_key, segment_parse};

/// The metadata fields that describe the recording itself. The rest of a metadata line is how
/// the audio was handled (device, power, transcription and speaker-analysis tooling, capture
/// mode, identifiers), and every reader of this header takes it as part of what was said.
const HEADER_FIELDS: &[&str] = &["language", "setting", "topics", "untimed"];

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let metadata = records
        .first()
        .filter(|record| !record.contains_key("start"));
    let mut skipped = 0usize;
    let mut chunks = Vec::new();
    let (base_timestamp, start_header) = base_timestamp_and_header(rel);

    for (index, record) in records.iter().enumerate() {
        if index == 0 && metadata.is_some() {
            continue;
        }
        if !record.contains_key("start") {
            skipped += 1;
            continue;
        }
        let start = record.get("start").map(display_value).unwrap_or_default();
        // The transcript writer stores a wall clock, already including segment
        // start. Anchor it to the nearest day so a small negative offset
        // stays before the segment while a midnight crossing wraps once.
        let timestamp = parse_clock(&start)
            .map(|clock| {
                let segment_clock = base_timestamp.rem_euclid(86_400_000);
                let delta = clock - segment_clock;
                let day_shift = if delta < -43_200_000 {
                    86_400_000
                } else if delta > 43_200_000 {
                    -86_400_000
                } else {
                    0
                };
                base_timestamp + delta + day_shift
            })
            .unwrap_or(base_timestamp);
        let mut parts = Vec::new();
        if !start.is_empty() {
            parts.push(format!("[{start}]"));
        }
        let source = record.get("source").map(display_value).unwrap_or_default();
        if !source.is_empty() {
            parts.push(format!("({source})"));
        }
        match record.get("speaker") {
            Some(Value::Number(value)) => parts.push(format!("Speaker {value}:")),
            Some(value) => parts.push(format!("{}:", display_value(value))),
            None => parts.push(String::new()),
        }

        let text = truthy_display(record, "corrected")
            .or_else(|| truthy_display(record, "text"))
            .unwrap_or_default();
        let prefix = parts.join(" ").trim().to_string();
        let mut markdown = if !prefix.is_empty() {
            if text.is_empty() {
                prefix
            } else {
                format!("{prefix} {text}")
            }
        } else if !text.is_empty() {
            text
        } else {
            continue;
        };
        let emotion = truthy_display(record, "emotion").unwrap_or_default();
        if !emotion.is_empty() && !emotion.eq_ignore_ascii_case("neutral") {
            markdown.push_str(&format!(" *({emotion})*"));
        }
        chunks.push(recorded_chunk(markdown, timestamp, record));
    }

    let header = audio_header(metadata, start_header);
    ProducedChunks {
        chunks,
        agent_override: Some("audio".to_string()),
        header,
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'start' field in {rel}")),
        warnings: Vec::new(),
    }
}

fn audio_header(metadata: Option<&JsonObject>, start_header: Option<String>) -> Option<String> {
    let mut parts = start_header.into_iter().collect::<Vec<_>>();
    if let Some(metadata) = metadata {
        for (key, value) in metadata {
            if !HEADER_FIELDS.contains(&key.as_str()) {
                continue;
            }
            let value = header_value(value);
            if !value.is_empty() {
                parts.push(format!("{}: {value}", capitalize(key)));
            }
        }
        if let Some(source) = audio_source(metadata) {
            parts.push(format!("Audio: {source}"));
        }
        if let Some(imported) = metadata.get("imported").and_then(Value::as_object) {
            if let Some(facet) = imported.get("facet").map(display_value)
                && !facet.is_empty()
            {
                parts.push(format!("Facet: {facet}"));
            }
            if let Some(id) = imported.get("id").map(display_value)
                && !id.is_empty()
            {
                parts.push(format!("Import ID: {id}"));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Where the audio came from, when the producer said: a desktop records the computer's own
/// sound (a call's other people, media) apart from the owner's microphone, and only the
/// microphone is the owner's voice.
fn audio_source(metadata: &JsonObject) -> Option<&'static str> {
    let kinds = metadata
        .get("audio_capture")?
        .get("sources")?
        .as_array()?
        .iter()
        .filter_map(|source| source.get("kind").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let system = kinds.contains(&"system");
    let microphone = kinds.contains(&"microphone");
    match (system, microphone) {
        (true, false) => Some(
            "this computer's own sound only, such as the other people on a call or media playing; \
             the owner's microphone was not recorded, so none of it is the owner speaking",
        ),
        (true, true) => Some("the owner's microphone and this computer's own sound, mixed"),
        (false, true) => Some("the owner's microphone"),
        (false, false) => None,
    }
}

fn base_timestamp_and_header(rel: &str) -> (i64, Option<String>) {
    let Some(day) = rel.split('/').find(|part| is_date_key(part)) else {
        return (0, None);
    };
    let Some(times) = segment_parse(rel) else {
        return (0, None);
    };
    let Some(day) = NaiveDate::parse_from_str(day, "%Y%m%d").ok() else {
        return (0, None);
    };
    let Some(start) = day.and_hms_opt(times.hour.into(), times.minute.into(), times.second.into())
    else {
        return (0, None);
    };
    (
        start.and_utc().timestamp_millis(),
        Some(start.format("Start: %Y-%m-%d %I:%M%P").to_string()),
    )
}

fn parse_clock(value: &str) -> Option<i64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = parts.next()?.parse::<i64>().ok()?;
    parts
        .next()
        .is_none()
        .then_some((hours * 3600 + minutes * 60 + seconds) * 1000)
}

fn header_value(value: &Value) -> String {
    match value {
        Value::Array(values) => values
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        _ => display_value(value),
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;

    #[test]
    fn published_absolute_clock_is_anchored_once() {
        // Captured from the physical common English fixture.
        let row = serde_json::json!({"start":"13:00:01","text":"Ravi explains that the blue water pump needs a new valve."});
        let produced = render(
            "20261012/device_audio/130001_15/audio.jsonl",
            &[row.as_object().unwrap().clone()],
        );
        let expected = NaiveDate::from_ymd_opt(2026, 10, 12)
            .unwrap()
            .and_hms_opt(13, 0, 1)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            produced.chunks[0].occurrence_time_ms.map(|v| v.0),
            Some(expected)
        );
    }

    #[test]
    fn a_negative_start_offset_stays_before_the_segment() {
        for (segment, clock, day, hour, minute, second) in [
            ("130001_15", "12:59:59", 12, 12, 59, 59),
            ("000001_15", "23:59:59", 11, 23, 59, 59),
        ] {
            let row = serde_json::json!({"start":clock,"text":"before segment"});
            let produced = render(
                &format!("20261012/device_audio/{segment}/audio.jsonl"),
                &[row.as_object().unwrap().clone()],
            );
            let expected = NaiveDate::from_ymd_opt(2026, 10, day)
                .unwrap()
                .and_hms_opt(hour, minute, second)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            assert_eq!(
                produced.chunks[0].occurrence_time_ms.map(|v| v.0),
                Some(expected)
            );
        }
    }

    #[test]
    fn a_published_clock_after_midnight_belongs_to_the_next_day() {
        let row = serde_json::json!({"start":"00:00:02","text":"after midnight"});
        let produced = render(
            "20261012/device_audio/235959_15/audio.jsonl",
            &[row.as_object().unwrap().clone()],
        );
        let expected = NaiveDate::from_ymd_opt(2026, 10, 13)
            .unwrap()
            .and_hms_opt(0, 0, 2)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(
            produced.chunks[0].occurrence_time_ms.map(|v| v.0),
            Some(expected)
        );
    }
}
