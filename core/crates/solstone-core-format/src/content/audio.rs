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

/// The speaker of a line from audio that holds only the computer's own sound: someone the journal
/// has not identified, and never the owner, because no microphone was recorded.
pub const OTHER_VOICE: &str = "Someone else";

/// Where a recording's sound came from, as its producer reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioSource {
    /// Only the computer's own sound: the other people on a call, media playing.
    Computer,
    Microphone,
    Both,
}

impl AudioSource {
    /// The header's word on it. A microphone alone is what every reader assumes, so it goes unsaid.
    fn describe(self) -> Option<&'static str> {
        match self {
            Self::Computer => {
                Some("this computer's own sound, such as a call or media, with no microphone")
            }
            Self::Microphone => None,
            Self::Both => Some("a microphone and this computer's own sound"),
        }
    }
}

/// The producer's report, from the most deliberate place it gives one: the `audio_sources` field,
/// then the macOS app's per-source capture record (sources that recorded nothing don't count), then
/// a split transcript's file name (`sys_audio`, `mic_audio`). `None` when the producer didn't say.
fn audio_source(rel: &str, metadata: Option<&JsonObject>) -> Option<AudioSource> {
    let declared = metadata.and_then(|metadata| {
        let kinds = if let Some(sources) = metadata.get("audio_sources").and_then(Value::as_array) {
            sources.iter().filter_map(Value::as_str).collect::<Vec<_>>()
        } else {
            metadata
                .get("audio_capture")?
                .get("sources")?
                .as_array()?
                .iter()
                .filter(|source| {
                    source.get("started").and_then(Value::as_bool) != Some(false)
                        && source.get("accepted_frames").and_then(Value::as_u64) != Some(0)
                })
                .filter_map(|source| source.get("kind").and_then(Value::as_str))
                .collect()
        };
        let computer = kinds
            .iter()
            .any(|kind| matches!(*kind, "computer" | "system"));
        let microphone = kinds
            .iter()
            .any(|kind| matches!(*kind, "microphone" | "mic"));
        match (computer, microphone) {
            (true, false) => Some(AudioSource::Computer),
            (false, true) => Some(AudioSource::Microphone),
            (true, true) => Some(AudioSource::Both),
            (false, false) => None,
        }
    });
    declared.or_else(|| match rel.rsplit('/').next()? {
        name if name.ends_with("sys_audio.jsonl") => Some(AudioSource::Computer),
        name if name.ends_with("mic_audio.jsonl") => Some(AudioSource::Microphone),
        _ => None,
    })
}

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let metadata = records
        .first()
        .filter(|record| !record.contains_key("start"));
    let mut skipped = 0usize;
    let mut chunks = Vec::new();
    let (base_timestamp, start_header) = base_timestamp_and_header(rel);
    let heard_from = audio_source(rel, metadata);

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
            // No microphone was recorded, so a line no voice was matched to is someone else's.
            None if heard_from == Some(AudioSource::Computer) => {
                parts.push(format!("{OTHER_VOICE}:"))
            }
            None => parts.push(String::new()),
        }

        let text = truthy_display(record, "corrected")
            .or_else(|| truthy_display(record, "text"))
            .unwrap_or_default();
        let prefix = parts.join(" ").trim().to_string();
        let markdown = if !prefix.is_empty() {
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
        chunks.push(recorded_chunk(markdown, timestamp, record));
    }

    let header = audio_header(metadata, start_header, heard_from);
    ProducedChunks {
        chunks,
        agent_override: Some("audio".to_string()),
        header,
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'start' field in {rel}")),
        warnings: Vec::new(),
    }
}

fn audio_header(
    metadata: Option<&JsonObject>,
    start_header: Option<String>,
    source: Option<AudioSource>,
) -> Option<String> {
    let mut parts = start_header.into_iter().collect::<Vec<_>>();
    if let Some(source) = source.and_then(AudioSource::describe) {
        parts.push(format!("Audio: {source}"));
    }
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
mod source_tests {
    use super::*;

    fn rows(header: serde_json::Value) -> Vec<JsonObject> {
        [
            header,
            serde_json::json!({"start":"05:50:00","text":"I'll start with beans."}),
            serde_json::json!({"start":"05:50:20","text":"Counting is enough.","speaker":2}),
        ]
        .into_iter()
        .map(|row| row.as_object().unwrap().clone())
        .collect()
    }

    fn mac(kinds: &[(&str, u64)]) -> serde_json::Value {
        let sources = kinds
            .iter()
            .map(|(kind, frames)| serde_json::json!({"kind":kind,"started":true,"accepted_frames":frames}))
            .collect::<Vec<_>>();
        serde_json::json!({"raw":"x.m4a","audio_capture":{"sources":sources}})
    }

    fn lines(produced: &ProducedChunks) -> Vec<String> {
        produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.clone())
            .collect()
    }

    #[test]
    fn computer_only_audio_is_said_and_its_unmatched_lines_are_someone_elses() {
        for (rel, header) in [
            (
                "20261031/device/055000_300/055000_300_audio.jsonl",
                mac(&[("system", 9)]),
            ),
            // The microphone was expected but recorded nothing.
            (
                "20261031/device/055000_300/055000_300_audio.jsonl",
                mac(&[("system", 9), ("microphone", 0)]),
            ),
            (
                "20261031/device/055000_300/055000_300_audio.jsonl",
                serde_json::json!({"audio_sources":["computer"]}),
            ),
            (
                "20261031/linux/055000_300/sys_audio.jsonl",
                serde_json::json!({"raw":"sys_audio.flac"}),
            ),
        ] {
            let produced = render(rel, &rows(header));
            assert!(
                produced
                    .header
                    .as_deref()
                    .unwrap()
                    .contains("Audio: this computer's own sound"),
                "{rel}"
            );
            let lines = lines(&produced);
            assert!(
                lines[0].contains("Someone else: I'll start with beans."),
                "{lines:?}"
            );
            // A voice the journal did tell apart keeps its own label.
            assert!(
                lines[1].contains("Speaker 2: Counting is enough."),
                "{lines:?}"
            );
        }
    }

    #[test]
    fn audio_with_a_microphone_keeps_unmatched_lines_unlabelled() {
        for (rel, header, said) in [
            (
                "20261031/device/055000_300/055000_300_audio.jsonl",
                mac(&[("system", 9), ("microphone", 9)]),
                Some("Audio: a microphone and this computer's own sound"),
            ),
            (
                "20261031/device/055000_300/055000_300_audio.jsonl",
                mac(&[("microphone", 9)]),
                None,
            ),
            (
                "20261031/linux/055000_300/mic_audio.jsonl",
                serde_json::json!({"raw":"mic_audio.flac"}),
                None,
            ),
            // A producer that doesn't say, such as a phone or watch.
            (
                "20261031/watch/055000_300/audio.jsonl",
                serde_json::json!({"raw":"audio.m4a"}),
                None,
            ),
        ] {
            let produced = render(rel, &rows(header));
            let header = produced.header.clone().unwrap_or_default();
            match said {
                Some(said) => assert!(header.contains(said), "{header}"),
                None => assert!(!header.contains("Audio:"), "{header}"),
            }
            assert!(
                !lines(&produced)
                    .iter()
                    .any(|line| line.contains(OTHER_VOICE))
            );
        }
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
