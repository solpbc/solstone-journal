// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Post-generation credential scrub over Sense output text fields before
//! `sense.json` / `activity.md` persistence and activity derivation.

use serde_json::{Map, Value};
use solstone_core_format::content::screen_text::redact_inline;

use crate::PreparedTalent;

/// Scrub credential shapes from `sense` output text fields, returning
/// `Some(rewritten)` when any field changed.
pub(crate) fn scrub(prepared: &PreparedTalent, response: &str) -> Option<String> {
    if prepared.name != "sense" {
        return None;
    }
    let mut output = serde_json::from_str::<Value>(response).ok()?;
    let object = output.as_object_mut()?;
    scrub_object(object).then(|| output.to_string())
}

/// Scrub credential shapes in place across Sense's persisted text fields:
/// `activity_summary`, `facets[].activity`, `entities[].context`, and
/// `quotes[].text`. Returns `true` when any field changed.
pub(crate) fn scrub_object(sense: &mut Map<String, Value>) -> bool {
    let mut changed = scrub_string_field(sense, "activity_summary");
    if let Some(facets) = sense.get_mut("facets").and_then(Value::as_array_mut) {
        for facet in facets.iter_mut().filter_map(Value::as_object_mut) {
            changed |= scrub_string_field(facet, "activity");
        }
    }
    if let Some(entities) = sense.get_mut("entities").and_then(Value::as_array_mut) {
        for entity in entities.iter_mut().filter_map(Value::as_object_mut) {
            changed |= scrub_string_field(entity, "context");
        }
    }
    if let Some(quotes) = sense.get_mut("quotes").and_then(Value::as_array_mut) {
        for quote in quotes.iter_mut().filter_map(Value::as_object_mut) {
            changed |= scrub_string_field(quote, "text");
        }
    }
    changed
}

fn scrub_string_field(object: &mut Map<String, Value>, key: &str) -> bool {
    let Some(Value::String(current)) = object.get_mut(key) else {
        return false;
    };
    let scrubbed = redact_inline(current);
    if scrubbed == *current {
        return false;
    }
    *current = scrubbed;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, writers::write_output_if_configured};
    use serde_json::json;

    #[test]
    fn sense_output_text_fields_are_scrubbed_before_persistence_and_activity_derivation() {
        let jwt =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let raw = json!({
            "density": "active",
            "content_type": "terminal",
            "activity_summary": "Unlocked vault with --password hunter2 on commit dbc2c5b8cf11395ac2842f300ea6f52f05c8216d.",
            "facets": [{
                "facet": "work",
                "level": "high",
                "activity": format!("Sent Authorization: Bearer {jwt} and --api-key sk-proj-abc123DEF456ghi789.")
            }],
            "entities": [{
                "type": "Tool",
                "name": "vault",
                "role": "mentioned",
                "source": "screen",
                "context": "Used ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5 and AKIAIOSFODNN7EXAMPLE in staging.",
                "level": "medium"
            }],
            "quotes": [{
                "speaker": "owner",
                "text": "Run cli --token mytok999 on branch feature-release-pipeline-cleanup"
            }],
            "speculative_facet": null,
            "meeting_detected": false,
            "speakers": [],
            "recommend": {"screen_record": false, "speaker_attribution": false},
            "topics": ["vault"]
        });

        let root = tempfile::tempdir().unwrap();
        let sense_path = root
            .path()
            .join("chronicle/20261124/090000_300/talents/sense.json");
        let prepared = PreparedTalent::new(
            "sense".into(),
            json!({
                "output": "json",
                "output_path": sense_path.to_string_lossy(),
            })
            .as_object()
            .unwrap()
            .clone(),
        );

        let scrubbed_str = scrub(&prepared, &raw.to_string()).expect("sense scrubbed");
        write_output_if_configured(
            &prepared,
            &ExecutionContext {
                journal: root.path().to_path_buf(),
            },
            &scrubbed_str,
        )
        .unwrap();

        let persisted_text = std::fs::read_to_string(&sense_path).unwrap();
        for leaked in [
            "hunter2",
            jwt,
            "sk-proj-abc123DEF456ghi789",
            "ghp_q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5",
            "AKIAIOSFODNN7EXAMPLE",
            "mytok999",
        ] {
            assert!(
                !persisted_text.contains(leaked),
                "persisted sense.json leaked {leaked}: {persisted_text}"
            );
        }
        assert!(persisted_text.contains("dbc2c5b8cf11395ac2842f300ea6f52f05c8216d"));
        assert!(persisted_text.contains("feature-release-pipeline-cleanup"));

        // Feed persisted sense.json into activity derivation and verify description is clean.
        let persisted: Value = serde_json::from_str(&persisted_text).unwrap();
        let mut machine = solstone_core_system::activity_state::ActivityStateMachine::default();
        let changes = machine.update(&persisted, "090000_300", "20261124", None, 1);
        let desc = changes[0]["description"].as_str().unwrap();
        assert!(!desc.contains(jwt), "{desc}");
        assert!(!desc.contains("sk-proj-abc123DEF456ghi789"), "{desc}");
        assert!(desc.contains("[REDACTED]"), "{desc}");
    }

    #[test]
    fn non_sense_talents_and_clean_prose_return_none() {
        let other = PreparedTalent::new("story".into(), Map::new());
        assert!(scrub(&other, r#"{"activity_summary":"--password hunter2"}"#).is_none());

        let sense = PreparedTalent::new("sense".into(), Map::new());
        let clean = json!({
            "activity_summary": "Reviewed commit dbc2c5b8cf11395ac2842f300ea6f52f05c8216d and updated docs.",
            "facets": [{"facet": "work", "level": "high", "activity": "Updated password manager docs."}],
            "entities": [{"context": "Discussed bearer bonds on the call."}],
            "quotes": [{"speaker": "owner", "text": "The build passed cleanly."}]
        });
        assert!(scrub(&sense, &clean.to_string()).is_none());
    }
}
