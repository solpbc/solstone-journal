// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::{Map, Value};

/// Activity records produced by planning are not evidence-bearing completed
/// activities and production deliberately does not dispatch talents for them.
pub fn is_synthetic(record: &Map<String, Value>) -> bool {
    record
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "cogitate" | "anticipated"))
}

/// Production's existing minimum span eligibility predicate.
pub fn has_nonempty_span(record: &Map<String, Value>) -> bool {
    record
        .get("segments")
        .and_then(Value::as_array)
        .is_some_and(|segments| !segments.is_empty())
}

/// Match one configured activity talent to the stored activity kind.
pub fn matches_activity(config: &Map<String, Value>, kind: &str) -> bool {
    config
        .get("activities")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item == "*" || item == kind)
        })
}

/// Production excludes low-confidence browsing/reading blocks from Work.
pub fn skips_low_level_work(name: &str, kind: &str, record: &Map<String, Value>) -> bool {
    name == "work"
        && matches!(kind, "browsing" | "reading")
        && record
            .get("level_avg")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            < 0.4
}

/// Production treats only an explicit `type: generate` activity talent as the
/// empty-prompt branch. Untyped talent configs are valid and receive the
/// activity prompt even though the runtime later defaults their engine to
/// Generate.
pub fn is_explicit_generate(config: &Map<String, Value>) -> bool {
    config.get("type").and_then(Value::as_str) == Some("generate")
}

/// The exact prompt production dispatch supplies to activity-scheduled
/// talents outside the explicit-generate branch.
pub fn cogitate_prompt(activity_id: &str, kind: &str, facet: &str, day: &str) -> String {
    let day = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| day.to_owned());
    format!("Processing activity '{activity_id}' ({kind}) in facet '{facet}' for {day}.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanFailure {
    Empty,
    Invalid,
}

/// Preview validates the whole stored span before the shared transcript loader
/// can filter malformed members and accidentally assemble a partial activity.
pub fn validated_span(record: &Map<String, Value>) -> Result<Vec<String>, SpanFailure> {
    let Some(segments) = record.get("segments") else {
        return Err(SpanFailure::Empty);
    };
    let Some(segments) = segments.as_array() else {
        return Err(SpanFailure::Invalid);
    };
    if segments.is_empty() {
        return Err(SpanFailure::Empty);
    }
    segments
        .iter()
        .map(|segment| {
            segment
                .as_str()
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .ok_or(SpanFailure::Invalid)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record(segments: Option<Value>) -> Map<String, Value> {
        segments
            .map(|segments| Map::from_iter([("segments".to_owned(), segments)]))
            .unwrap_or_default()
    }

    #[test]
    fn validated_span_rejects_every_partial_or_empty_shape() {
        for (segments, expected) in [
            (None, SpanFailure::Empty),
            (Some(json!([])), SpanFailure::Empty),
            (Some(json!("090000_60")), SpanFailure::Invalid),
            (Some(json!(["090000_60", 7])), SpanFailure::Invalid),
            (Some(json!(["090000_60", ""])), SpanFailure::Invalid),
        ] {
            assert_eq!(validated_span(&record(segments)), Err(expected));
        }
    }

    #[test]
    fn validated_span_preserves_every_valid_member_in_order() {
        assert_eq!(
            validated_span(&record(Some(json!(["100000_60", "090000_60"])))),
            Ok(vec!["100000_60".to_owned(), "090000_60".to_owned()])
        );
    }

    #[test]
    fn cogitate_prompt_preserves_the_production_sentence_and_day_fallback() {
        assert_eq!(
            cogitate_prompt("reading_1", "reading", "work", "20260813"),
            "Processing activity 'reading_1' (reading) in facet 'work' for 2026-08-13."
        );
        assert_eq!(
            cogitate_prompt("A", "meeting", "home", "not-a-day"),
            "Processing activity 'A' (meeting) in facet 'home' for not-a-day."
        );
    }

    #[test]
    fn only_explicit_generate_uses_the_empty_activity_prompt_branch() {
        assert!(is_explicit_generate(&Map::from_iter([(
            "type".to_owned(),
            json!("generate")
        )])));
        assert!(!is_explicit_generate(&Map::new()));
        assert!(!is_explicit_generate(&Map::from_iter([(
            "type".to_owned(),
            json!("cogitate")
        )])));
    }
}
