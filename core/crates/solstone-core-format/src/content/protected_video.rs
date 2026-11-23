// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_window_catalog::{ENGLISH, Value as CatalogValue, render as render_msg, strings};

use super::{JsonObject, ProducedChunks, recorded_chunk_for_locale};

#[allow(dead_code)]
pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(records, ENGLISH)
}

pub(super) fn render_for_locale(records: &[JsonObject], locale: &str) -> ProducedChunks {
    let mut chunks = Vec::new();

    for record in records {
        if record.get("t").and_then(|v| v.as_str()) != Some("protected_video") {
            continue;
        }

        let Some(ts) = record
            .get("ts")
            .and_then(super::browser::integer_milliseconds)
        else {
            continue;
        };
        let Some(end_ts) = record
            .get("end_ts")
            .and_then(super::browser::integer_milliseconds)
        else {
            continue;
        };
        if end_ts < ts {
            continue;
        }

        let Some(service) = record
            .get("service")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        let app = record
            .get("app")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let title = record
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let span = i128::from(end_ts) - i128::from(ts);
        let minutes = i64::try_from(((span + 30_000) / 60_000).max(1)).expect("minute count");
        let duration = render_msg(
            locale,
            "journal.window.content.protected.video.minutes",
            &[("quantity", CatalogValue::Cardinal(minutes))],
        );

        let heading = strings(
            locale,
            "journal.window.content.protected.video.heading",
            [("service", service)],
        );
        let body = match (app, title) {
            (Some(app), Some(title)) => strings(
                locale,
                "journal.window.content.protected.video.body.app.title",
                [
                    ("service", service),
                    ("app", app),
                    ("title", title),
                    ("duration", duration.as_str()),
                ],
            ),
            (Some(app), None) => strings(
                locale,
                "journal.window.content.protected.video.body.app",
                [
                    ("service", service),
                    ("app", app),
                    ("duration", duration.as_str()),
                ],
            ),
            (None, Some(title)) => strings(
                locale,
                "journal.window.content.protected.video.body.title",
                [
                    ("service", service),
                    ("title", title),
                    ("duration", duration.as_str()),
                ],
            ),
            (None, None) => strings(
                locale,
                "journal.window.content.protected.video.body",
                [("service", service), ("duration", duration.as_str())],
            ),
        };
        let prose = format!("{heading}\n\n{body}");

        chunks.push(recorded_chunk_for_locale(prose, ts, record, locale));
    }

    ProducedChunks {
        chunks,
        agent_override: Some("protected_video".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{
        ContentResolution, Family, IndexDisposition, RawPerceptFamily, classify, produce_chunks,
        resolve_spec,
    };

    #[test]
    fn protected_video_fixtures_differ_and_match_contract() {
        let fixture_a = serde_json::json!({
            "t": "protected_video",
            "ts": 1_792_027_800_000_i64,
            "end_ts": 1_792_030_500_000_i64,
            "service": "Netflix",
            "app": "Comet",
            "title": "Netflix",
        });
        let record_a = fixture_a.as_object().unwrap().clone();
        let produced_a = render(&[record_a]);

        assert_eq!(produced_a.chunks.len(), 1);
        let text_a = &produced_a.chunks[0].content;
        assert!(text_a.contains("Netflix"));
        assert!(text_a.contains("45"));
        assert!(text_a.contains("Comet"));

        let fixture_b = serde_json::json!({
            "t": "protected_video",
            "ts": 1_000_i64,
            "end_ts": 91_000_i64,
            "service": "Apple TV",
        });
        let record_b = fixture_b.as_object().unwrap().clone();
        let produced_b = render(&[record_b]);

        assert_eq!(produced_b.chunks.len(), 1);
        let text_b = &produced_b.chunks[0].content;
        assert!(text_b.contains("Apple TV"));
        assert!(text_b.contains("2"));
        assert!(!text_b.contains("Netflix"));
        assert!(!text_b.contains("Comet"));

        assert_eq!(
            produced_a.chunks[0].occurrence_time_ms.unwrap().0,
            1_792_027_800_000
        );
        assert_eq!(
            produced_a.agent_override.as_deref(),
            Some("protected_video")
        );
        assert_eq!(produced_a.error, None);
        assert!(produced_a.warnings.is_empty());

        let empty_produced = render(&[]);
        assert!(empty_produced.chunks.is_empty());
        assert_eq!(
            empty_produced.agent_override.as_deref(),
            Some("protected_video")
        );
        assert_eq!(empty_produced.error, None);
        assert!(empty_produced.warnings.is_empty());
    }

    #[test]
    fn protected_video_mixed_text_skips_invalid_rows_and_handles_integral_floats() {
        let lines = [
            // 1. Valid A
            r#"{"t":"protected_video","ts":1792027800000,"end_ts":1792030500000,"service":"Netflix","app":"Comet","title":"Netflix"}"#,
            // 2. Truncated JSON line
            r#"{"t":"protected_video""#,
            // 3. Line 42
            r#"42"#,
            // 4. Row with end_ts < ts
            r#"{"t":"protected_video","ts":2000,"end_ts":1000,"service":"Hulu"}"#,
            // 5. Row with empty service
            r#"{"t":"protected_video","ts":1000,"end_ts":2000,"service":"   "}"#,
            // 6. Valid row with ts as 1792027800000.0 (integral float)
            r#"{"t":"protected_video","ts":1792027800000.0,"end_ts":1792027860000,"service":"Disney"}"#,
            // 7. Valid B
            r#"{"t":"protected_video","ts":1000,"end_ts":91000,"service":"Apple TV"}"#,
        ];
        let content = lines.join("\n");
        let rel = "20260910/desk/120000_60/120000_60_protected_video.jsonl";
        let produced = produce_chunks(Family::ProtectedVideo, rel, &content);

        assert_eq!(produced.chunks.len(), 3);
        assert_eq!(
            produced.chunks[0].occurrence_time_ms.unwrap().0,
            1_792_027_800_000
        );
        assert_eq!(
            produced.chunks[1].occurrence_time_ms.unwrap().0,
            1_792_027_800_000
        );
        assert!(produced.chunks[1].content.contains("Disney"));
        assert_eq!(produced.chunks[2].occurrence_time_ms.unwrap().0, 1_000);
    }

    #[test]
    fn protected_video_classification_and_disposition() {
        let note_path = "20260910/desk/120000_60/120000_60_protected_video.jsonl";
        assert_eq!(
            classify(note_path),
            ContentResolution::Indexed(Family::ProtectedVideo)
        );
        let spec = resolve_spec(note_path).expect("spec resolves");
        assert_eq!(spec.family, Family::ProtectedVideo);
        assert_eq!(spec.disposition, IndexDisposition::Excluded);

        let screen_path = "20260910/desk/120000_60/x_screen.jsonl";
        assert_eq!(
            classify(screen_path),
            ContentResolution::Unindexed(RawPerceptFamily::RawScreen)
        );

        let browser_path = "20260910/desk/120000_60/browser_x.jsonl";
        assert_eq!(
            classify(browser_path),
            ContentResolution::Indexed(Family::Browser)
        );

        assert_eq!(
            classify("protected_video.jsonl"),
            ContentResolution::Unrecognized
        );
        assert_eq!(
            classify("20260910/desk/120000_60/protected_video.jsonl"),
            ContentResolution::Unrecognized
        );
    }
}
