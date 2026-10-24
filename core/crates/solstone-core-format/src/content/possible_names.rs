// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names a segment's audio may refer to, as search content.
//!
//! The possible-names talent writes `talents/possible_names.json` beside a
//! segment's transcript. `solstone-core-possible-names` owns that contract and
//! decides, every time it is read, which entries may still be shown. Search
//! holds only the labelled canonical name: one chunk per entry, in entry
//! order, reading `possible name: <name>`. The transcript words an entry
//! labels never enter the index.

use serde_json::Value;

use super::{IndexChunk, JsonObject, ProducedChunks};

/// The schema every possible-names sidecar declares.
pub const POSSIBLE_NAMES_SCHEMA: &str = "solstone-possible-names-v1";
/// The sidecar's path inside its segment directory.
pub const POSSIBLE_NAMES_RELATIVE_PATH: &str = "talents/possible_names.json";
/// What every possible-name label starts with, wherever it is shown.
pub const POSSIBLE_NAME_LABEL: &str = "possible name: ";

/// The words that label a possible name, as search content and inline.
#[must_use]
pub fn possible_name_label(name: &str) -> String {
    format!("{POSSIBLE_NAME_LABEL}{name}")
}

/// Whether a stored search path is a possible-names sidecar.
pub fn is_possible_names_path(rel: &str) -> bool {
    let normalized = rel.strip_prefix("chronicle/").unwrap_or(rel);
    normalized.ends_with(POSSIBLE_NAMES_RELATIVE_PATH)
        && super::resolve_spec(normalized)
            .is_some_and(|spec| spec.family == super::Family::PossibleNames)
}

/// One chunk per entry, so chunk `idx` is entry `idx`. A sidecar that does not
/// declare the schema, or has an entry without a name, renders nothing rather
/// than a misaligned list.
pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .first()
        .and_then(entry_names)
        .unwrap_or_default()
        .into_iter()
        .map(|name| IndexChunk {
            content: possible_name_label(name),
            occurrence_time_ms: None,
            source: None,
            group_offset_ms: None,
        })
        .collect();
    ProducedChunks {
        chunks,
        agent_override: Some("possible_names".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn entry_names(sidecar: &JsonObject) -> Option<Vec<&str>> {
    if sidecar.get("schema").and_then(Value::as_str) != Some(POSSIBLE_NAMES_SCHEMA) {
        return None;
    }
    sidecar
        .get("entries")?
        .as_array()?
        .iter()
        .map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{
        AdmittedCategory, ContentResolution, Family, IndexDisposition, ScopeBasis, classify,
        produce_chunks, resolve_spec,
    };
    use super::{POSSIBLE_NAMES_SCHEMA, is_possible_names_path};

    const REL: &str = "20261024/default/090000_300/talents/possible_names.json";

    #[test]
    fn the_sidecar_is_segment_transcript_content() {
        assert_eq!(
            classify(REL),
            ContentResolution::Indexed(Family::PossibleNames)
        );
        assert_eq!(
            resolve_spec(REL).unwrap().disposition,
            IndexDisposition::Admitted {
                category: AdmittedCategory::Transcripts,
                basis: ScopeBasis::SegmentAssigned,
            }
        );
        assert!(is_possible_names_path(REL));
        assert!(is_possible_names_path(&format!("chronicle/{REL}")));
        assert!(!is_possible_names_path(
            "20261024/default/090000_300/talents/screen_text.md"
        ));
        assert!(!is_possible_names_path(
            "20261024/talents/possible_names.json"
        ));
    }

    #[test]
    fn one_chunk_per_entry_holds_the_name_and_never_what_was_heard() {
        let sidecar = json!({
            "schema": POSSIBLE_NAMES_SCHEMA,
            "talent_version": "1",
            "source": {"file": "audio.jsonl", "sha256": "00"},
            "registry_revision": "r",
            "generated_at_ms": 1,
            "entries": [
                {"sentence": 1, "start_byte": 0, "end_byte": 5, "heard": "Fryer", "entity_id": "friar", "name": "Friar"},
                {"sentence": 2, "start_byte": 4, "end_byte": 8, "heard": "anna", "entity_id": "ana", "name": "Ana Lima"}
            ]
        });
        let produced = produce_chunks(Family::PossibleNames, REL, &sidecar.to_string());
        let contents = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            contents,
            ["possible name: Friar", "possible name: Ana Lima"]
        );
        assert!(produced.chunks.iter().all(|chunk| chunk.source.is_none()));
        assert!(!contents.iter().any(|content| content.contains("Fryer")));
    }

    #[test]
    fn a_sidecar_off_contract_renders_nothing() {
        for text in [
            "not json".to_owned(),
            json!({"schema": "other", "entries": [{"name": "Friar"}]}).to_string(),
            json!({"schema": POSSIBLE_NAMES_SCHEMA, "entries": [{"name": "Friar"}, {"name": " "}]})
                .to_string(),
            json!({"schema": POSSIBLE_NAMES_SCHEMA, "entries": [{"entity_id": "friar"}]})
                .to_string(),
        ] {
            assert!(
                produce_chunks(Family::PossibleNames, REL, &text)
                    .chunks
                    .is_empty(),
                "{text}"
            );
        }
        let empty = json!({"schema": POSSIBLE_NAMES_SCHEMA, "entries": []}).to_string();
        assert!(
            produce_chunks(Family::PossibleNames, REL, &empty)
                .chunks
                .is_empty()
        );
    }

    #[test]
    fn talent_inputs_never_read_possible_names() {
        // Talents and the owner's talent tabs read a segment's outputs by
        // name within its own talents directory, as production does.
        let talents = crate::test_support::reserve_temp_path("possible-names-projection");
        std::fs::create_dir(&talents).unwrap();
        std::fs::write(
            talents.join("possible_names.json"),
            json!({"schema": POSSIBLE_NAMES_SCHEMA, "entries": [{"name": "Friar"}]}).to_string(),
        )
        .unwrap();
        std::fs::write(talents.join("brief.md"), "a brief").unwrap();
        let projections = super::super::iter_talent_text_projections(&talents, "", None).unwrap();
        assert_eq!(
            projections
                .iter()
                .map(|projection| projection.stem.as_str())
                .collect::<Vec<_>>(),
            ["brief"]
        );
        std::fs::remove_dir_all(talents).unwrap();
    }
}
