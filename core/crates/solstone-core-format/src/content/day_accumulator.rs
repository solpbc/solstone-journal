// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, recorded_chunk};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .iter()
        .filter_map(|record| {
            serde_json::to_string(record).ok().map(|content| {
                recorded_chunk(
                    content,
                    record.get("ts").and_then(Value::as_i64).unwrap_or(0),
                    record,
                )
            })
        })
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some(file_stem(rel).to_lowercase()),
        header: None,
        warnings: Vec::new(),
    }
}

fn file_stem(rel: &str) -> &str {
    let filename = rel.rsplit('/').next().unwrap_or(rel);
    filename
        .rsplit_once('.')
        .map(|(stem, _extension)| stem)
        .unwrap_or(filename)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn renders_one_serialized_chunk_per_object() {
        let records = parse_jsonl_objects(
            r#"{"ts":123,"summary":"hello","ok":true}
{}
42
"#,
        );
        let produced = render("20260304/talents/Pulse.jsonl", &records);

        assert_eq!(produced.agent_override.as_deref(), Some("pulse"));
        assert_eq!(produced.chunks.len(), 2);
        assert!(produced.chunks[0].content.contains(r#""summary":"hello""#));
        assert!(produced.chunks[0].content.contains(r#""ok":true"#));
        assert_eq!(produced.chunks[1].content, "{}");
    }
}
