// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{ChatLabels, Family};
use super::{
    JsonObject, RawPerceptFamily, produce_chunks_by_shape, produce_raw_percept_chunks_by_shape,
};

/// Render already-parsed raw-screen records as the owner-facing text stream.
pub fn render_raw_screen_text(rel: &str, records: &[JsonObject]) -> String {
    let produced =
        produce_raw_percept_chunks_by_shape(RawPerceptFamily::RawScreen, Some(rel), records);
    let mut parts = Vec::new();
    if let Some(header) = produced.header {
        parts.push(header);
    }
    parts.extend(produced.chunks.into_iter().map(|chunk| chunk.content));
    parts.join("\n")
}

/// Render already-parsed browser records as the owner-facing text stream.
pub fn render_browser_text(records: &[JsonObject]) -> String {
    let produced = produce_chunks_by_shape(Family::Browser, None, records, &ChatLabels::default());
    produced
        .chunks
        .into_iter()
        .map(|chunk| chunk.content)
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    fn case_by_id<'a>(fixture: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        fixture["cases"]
            .as_array()
            .expect("fixture cases")
            .iter()
            .find(|case| case["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("fixture case {id}"))
    }

    #[test]
    fn raw_screen_text_includes_its_recorded_header_with_single_newline_separators() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/content_families.json"))
                .expect("content family fixture parses");
        let case = case_by_id(&fixture, "screen_frames_nominal");
        let records = parse_jsonl_objects(case["input_text"].as_str().expect("input text"));
        let expected_chunks = case["chunks"]
            .as_array()
            .expect("chunks")
            .iter()
            .map(|chunk| chunk["markdown"].as_str().expect("markdown"))
            .collect::<Vec<_>>();
        let expected = std::iter::once(case["header"].as_str().expect("header"))
            .chain(expected_chunks)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            render_raw_screen_text(case["rel"].as_str().expect("rel"), &records),
            expected
        );
    }

    #[test]
    fn browser_text_excludes_headers_with_double_newline_separators() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/content_families.json"))
                .expect("content family fixture parses");
        let case = case_by_id(&fixture, "browser_snapshot_then_delta");
        let records = parse_jsonl_objects(case["input_text"].as_str().expect("input text"));
        let expected = case["chunks"]
            .as_array()
            .expect("chunks")
            .iter()
            .map(|chunk| chunk["markdown"].as_str().expect("markdown"))
            .collect::<Vec<_>>()
            .join("\n\n");

        assert!(case["header"].is_null());
        assert_eq!(render_browser_text(&records), expected);
    }
}
