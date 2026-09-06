// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-segment entity detection hook.

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};
use serde_json::{Map, Value};
use solstone_core_system_health::find_segment_dir;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

pub const NOTABILITY_LABELS: [(&str, &str); 3] = [
    ("high", "This was a main focus"),
    ("medium", "This came up clearly"),
    ("low", "This came up in passing"),
];
pub const CENTRALITY_LABELS: [(&str, &str); 3] = [
    ("high", "central to this moment"),
    ("medium", "meaningfully involved"),
    ("low", "a peripheral mention"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct DetectionState {
    packet: String,
}
fn segment(prepared: &PreparedTalent, journal: &std::path::Path) -> Option<std::path::PathBuf> {
    find_segment_dir(
        journal,
        prepared.config.get("day")?.as_str()?,
        prepared.config.get("segment")?.as_str()?,
        prepared.config.get("stream").and_then(Value::as_str),
    )
}
fn read_sense(path: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path.join("talents/sense.json")).ok()?)
        .ok()
        .filter(Value::is_object)
}
fn segment_facets(sense: &Value) -> Vec<Map<String, Value>> {
    sense
        .get("facets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|row| {
            row.get("facet")
                .and_then(Value::as_str)
                .is_some_and(|facet| !facet.is_empty())
        })
        .cloned()
        .collect()
}
fn candidate_rows(sense: &Value) -> Vec<Map<String, Value>> {
    sense
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect()
}
pub fn known_lines_for_active_facets(
    journal: &std::path::Path,
    name: &str,
    facets: &[Map<String, Value>],
) -> Vec<String> {
    let mut lines = Vec::new();
    for row in facets {
        let Some(facet) = row.get("facet").and_then(Value::as_str) else {
            continue;
        };
        let Ok(scoped) =
            solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        else {
            continue;
        };
        let candidates = scoped
            .iter()
            .map(
                |entity| solstone_core_entity_matching::EntityNameCandidate {
                    id: Some(entity.entity_id.clone()),
                    name: entity
                        .identity
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    aka: Vec::new(),
                    emails: Vec::new(),
                },
            )
            .collect::<Vec<_>>();
        let Some(matched) =
            solstone_core_entity_matching::find_matching_entity(name, &candidates, 90.0)
        else {
            continue;
        };
        let entity_id = candidates[matched.candidate_index]
            .id
            .as_deref()
            .unwrap_or_default();
        let Ok(relationships) = solstone_core_facets::load_all_facet_relationships(journal, facet)
        else {
            continue;
        };
        let Some(description) = relationships
            .get(entity_id)
            .and_then(|value| value.get("description"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        lines.push(format!("- In {facet}: {description}"));
    }
    if lines.is_empty() {
        vec!["- No saved notes for the active facets.".to_owned()]
    } else {
        lines
    }
}
pub fn daily_summary_lines(
    journal: &std::path::Path,
    day: &str,
    name: &str,
    facets: &[Map<String, Value>],
) -> Vec<String> {
    let slug = solstone_core_entity_matching::entity_slug(name);
    let mut lines = Vec::new();
    for row in facets {
        let Some(facet) = row.get("facet").and_then(Value::as_str) else {
            continue;
        };
        let Ok(entities) = solstone_core_facets::read_detected_entities(journal, facet, day) else {
            continue;
        };
        for entity in entities {
            let id = entity
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    solstone_core_entity_matching::entity_slug(
                        entity
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                });
            if id != slug {
                continue;
            }
            if let Some(description) = entity
                .get("description")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                lines.push(format!("Summary so far today in {facet}: {description}"));
            }
            break;
        }
    }
    if lines.is_empty() {
        vec!["Summary so far today: Nothing saved yet in the active facets.".to_owned()]
    } else {
        lines
    }
}
pub fn build_packet(
    journal: &std::path::Path,
    day: &str,
    facets: &[Map<String, Value>],
    candidates: &[Map<String, Value>],
) -> String {
    let mut lines = vec![
        "This is a moment from today. You keep a running daily log of who and what mattered, organized by facet.".to_owned(),
        String::new(),
        "## Facets active in this moment".to_owned(),
        String::new(),
    ];
    for row in facets {
        let facet = row.get("facet").and_then(Value::as_str).unwrap_or_default();
        let description = solstone_core_facets::read_facet_declaration(journal, facet)
            .ok()
            .flatten()
            .map(|value| value.description)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "No description saved.".to_owned());
        let activity = row
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or_default();
        lines.extend([
            format!("### {facet}"),
            format!("Facet: {description}"),
            format!("What happened here: {activity}"),
            format!(
                "Why it matters: {}.",
                notability_label(row.get("level").unwrap_or(&Value::Null))
            ),
            String::new(),
        ]);
    }
    lines.extend(["## People and things noticed".to_owned(), String::new()]);
    for candidate in candidates {
        let name = candidate
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            continue;
        }
        let context = candidate
            .get("context")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        lines.extend([format!("### {name}"), "What's known:".to_owned()]);
        lines.extend(known_lines_for_active_facets(journal, name, facets));
        lines.extend(daily_summary_lines(journal, day, name, facets));
        lines.push(format!(
            "In this moment: {}",
            if context.is_empty() {
                "No one-line activity was provided."
            } else {
                context
            }
        ));
        if let Some(cue) = centrality_cue(candidate.get("level").unwrap_or(&Value::Null)) {
            lines.push(format!("How central it was: {cue}."));
        }
        lines.push(String::new());
    }
    lines.join("\n").trim().to_owned() + "\n"
}
pub fn notability_label(raw_level: &Value) -> &'static str {
    NOTABILITY_LABELS
        .iter()
        .find_map(|(level, label)| {
            (raw_level.to_string().trim_matches('"') == *level).then_some(*label)
        })
        .unwrap_or("This came up")
}
pub fn centrality_cue(raw_level: &Value) -> Option<&'static str> {
    CENTRALITY_LABELS.iter().find_map(|(level, cue)| {
        (raw_level.to_string().trim_matches('"') == *level).then_some(*cue)
    })
}
pub fn gate(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let Some(path) = segment(prepared, &context.journal) else {
        return Ok(GateDecision::Skip("no_sense".to_owned()));
    };
    let Some(sense) = read_sense(&path) else {
        return Ok(GateDecision::Skip("no_sense".to_owned()));
    };
    if segment_facets(&sense).is_empty() {
        return Ok(GateDecision::Skip("no_facets".to_owned()));
    }
    if candidate_rows(&sense).is_empty() {
        return Ok(GateDecision::Skip("no_candidates".to_owned()));
    }
    Ok(GateDecision::Proceed)
}
pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let path = segment(prepared, &context.journal).ok_or_else(|| RuntimeOutcome::Skipped {
        stage: "entities:detection".to_owned(),
        talent: prepared.name.clone(),
        reason: "no_sense".to_owned(),
    })?;
    let value = read_sense(&path).unwrap_or(Value::Null);
    Ok(PrePostState::EntityDetection(DetectionState {
        packet: build_packet(
            &context.journal,
            prepared
                .config
                .get("day")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &segment_facets(&value),
            &candidate_rows(&value),
        ),
    }))
}
pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntityDetection(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:detection",
            prepared,
            "missing detection state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "detection_packet".to_owned(),
            Value::String(state.packet.clone()),
        )]),
    );
    Ok(())
}
pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}
pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:detection",
            prepared,
            "expected text output",
        ));
    };
    let (Some(day), Some(segment)) = (
        prepared.config.get("day").and_then(Value::as_str),
        prepared.config.get("segment").and_then(Value::as_str),
    ) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::EntityDetection {
        output,
        day: day.to_owned(),
        segment: segment.to_owned(),
        stream: prepared
            .config
            .get("stream")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }))
}
pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    day: &str,
    segment_name: &str,
    stream: Option<&str>,
) -> Result<(), String> {
    let Some(path) = find_segment_dir(journal, day, segment_name, stream) else {
        return Ok(());
    };
    let outcome = path.join("talents/detection_outcome.json");
    let (counts, error) = apply_detections(journal, output, day, &path);
    let payload = serde_json::json!({
        "wrote": counts.wrote,
        "skipped": counts.skipped,
        "dropped": counts.dropped,
        "errored": counts.errored,
        "error": error,
        "ts": chrono::Utc::now().timestamp_millis(),
    });
    let _ = fs::write(outcome, format!("{payload}\n"));
    Ok(())
}

#[derive(Default)]
struct DetectionCounts {
    wrote: usize,
    skipped: usize,
    dropped: usize,
    errored: usize,
}

fn apply_detections(
    journal: &std::path::Path,
    output: &str,
    day: &str,
    path: &std::path::Path,
) -> (DetectionCounts, Option<String>) {
    let mut counts = DetectionCounts::default();
    let mut error = None;
    let origin = solstone_core_journal_io::segment_origin(path);
    let sense = read_sense(path).unwrap_or(Value::Null);
    let facets = sense
        .get("facets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row.get("facet").and_then(Value::as_str).map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let types = sense
        .get("entities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let name = row.get("name")?.as_str()?.trim();
            (!name.is_empty()).then(|| {
                (
                    solstone_core_entity_matching::entity_slug(name),
                    row.get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let Ok(Value::Object(value)) = serde_json::from_str(output) else {
        return (counts, error);
    };
    let Some(rows) = value.get("detections").and_then(Value::as_array) else {
        return (counts, error);
    };
    let mut kept_by_facet =
        BTreeMap::<String, Vec<solstone_core_facets::DetectedEntityInput>>::new();
    for row in rows {
        let Some(row) = row.as_object() else {
            counts.dropped += 1;
            continue;
        };
        let (Some(name), Some(facet), Some(description)) = (
            row.get("name").and_then(Value::as_str),
            row.get("facet").and_then(Value::as_str),
            row.get("description").and_then(Value::as_str),
        ) else {
            counts.dropped += 1;
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !facets.contains(facet) {
            counts.dropped += 1;
            continue;
        }
        let Some(entity_type) = types
            .get(&solstone_core_entity_matching::entity_slug(name))
            .cloned()
        else {
            counts.dropped += 1;
            continue;
        };
        kept_by_facet.entry(facet.to_owned()).or_default().push(
            solstone_core_facets::DetectedEntityInput {
                entity_type,
                name: name.to_owned(),
                description: description.to_owned(),
            },
        );
    }
    for (facet, detections) in kept_by_facet {
        match solstone_core_facets::upsert_detection_segment(
            journal,
            &facet,
            day,
            &origin,
            &detections,
        ) {
            Ok(report) => counts.wrote += report.wrote,
            Err(upsert_error) => {
                counts.errored += 1;
                error = Some(format!("FacetEntityWriteError: {upsert_error}"));
            }
        }
    }
    (counts, error)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_segment_is_non_creating() {
        // Derived from solstone/apps/entities/talent/detection.py:52-58 and solstone/think/cluster.py:749-764.
        let root = tempfile::tempdir().unwrap();
        assert!(find_segment_dir(root.path(), "20260101", "090000_60", None).is_none());
        assert!(!root.path().join("chronicle").exists());
    }
    #[test]
    fn maps_notability_and_centrality_levels() {
        // Derived from solstone/apps/entities/talent/detection.py:79-85.
        for (level, label, cue) in [
            ("high", "This was a main focus", "central to this moment"),
            ("medium", "This came up clearly", "meaningfully involved"),
            ("low", "This came up in passing", "a peripheral mention"),
        ] {
            assert_eq!(notability_label(&Value::String(level.to_owned())), label);
            assert_eq!(centrality_cue(&Value::String(level.to_owned())), Some(cue));
        }
        assert_eq!(
            notability_label(&Value::String("other".to_owned())),
            "This came up"
        );
        assert_eq!(centrality_cue(&Value::String("other".to_owned())), None);
    }
    #[test]
    fn reads_sense_and_selects_reference_rows() {
        // Derived from solstone/apps/entities/talent/detection.py:44-78.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("segment");
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(
            path.join("talents/sense.json"),
            r#"{"facets":[{"facet":"work"},{"facet":""},"bad"],"entities":[{"name":"Ada"},"bad"]}"#,
        )
        .unwrap();
        let sense = read_sense(&path).unwrap();
        assert_eq!(segment_facets(&sense).len(), 1);
        assert_eq!(candidate_rows(&sense).len(), 1);
        assert!(segment_facets(&Value::Object(Map::new())).is_empty());
        assert!(candidate_rows(&Value::Object(Map::new())).is_empty());
        assert!(read_sense(root.path()).is_none());
    }
    #[test]
    fn entity_lookup_lines_keep_reference_fallbacks() {
        // Derived from solstone/apps/entities/talent/detection.py:87-132.
        let root = tempfile::tempdir().unwrap();
        let facets = vec![Map::from_iter([(
            "facet".to_owned(),
            Value::String("work".to_owned()),
        )])];
        assert_eq!(
            known_lines_for_active_facets(root.path(), "Ada", &facets),
            vec!["- No saved notes for the active facets."]
        );
        assert_eq!(
            daily_summary_lines(root.path(), "20260101", "Ada", &facets),
            vec!["Summary so far today: Nothing saved yet in the active facets."]
        );
    }
    #[test]
    fn packet_has_reference_sections() {
        // Derived from solstone/apps/entities/talent/detection.py:133-186.
        let root = tempfile::tempdir().unwrap();
        let facets = vec![Map::from_iter([
            ("facet".to_owned(), Value::String("work".to_owned())),
            ("activity".to_owned(), Value::String("reviewed".to_owned())),
            ("level".to_owned(), Value::String("high".to_owned())),
        ])];
        let candidates = vec![Map::from_iter([
            ("name".to_owned(), Value::String("Ada".to_owned())),
            ("context".to_owned(), Value::String("planned".to_owned())),
        ])];
        let packet = build_packet(root.path(), "20260101", &facets, &candidates);
        assert!(
            packet.contains(
                "## Facets active in this moment\n\n### work\nFacet: No description saved."
            )
        );
        assert!(packet.contains("## People and things noticed\n\n### Ada\nWhat's known:"));
    }

    #[test]
    fn post_upserts_detected_entities_and_writes_outcome() {
        // Derived from solstone/apps/entities/talent/detection.py:222-310.
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::create_dir_all(root.path().join("entities/ada")).unwrap();
        fs::write(
            root.path().join("entities/ada/entity.json"),
            r#"{"id":"ada","name":"Ada","type":"Person"}"#,
        )
        .unwrap();
        fs::write(segment.join("talents/sense.json"), r#"{"facets":[{"facet":"work"}],"entities":[{"name":"Ada","type":"Person"},{"name":"Unmatched","type":"Tool"}]}"#).unwrap();
        apply_result(root.path(), r#"{"detections":[{"name":"Ada","facet":"work","description":"Known person."},{"name":"Unmatched","facet":"work","description":"Known tool."}]}"#, "20260101", "090000_300", None).unwrap();
        let rows =
            solstone_core_facets::read_detected_entities(root.path(), "work", "20260101").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row["id"] == "ada"));
        assert!(segment.join("talents/detection_outcome.json").is_file());
    }

    #[test]
    fn post_uses_slug_identity_and_records_dropped_rows() {
        // Derived from solstone/apps/entities/talent/detection.py:235-306.
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/090000_300/talents");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("sense.json"),
            r#"{"facets":[{"facet":"work"}],"entities":[{"name":"Müller & Co.","type":"Company"}]}"#,
        )
        .unwrap();
        apply_result(
            root.path(),
            r#"{"detections":[{"name":"Müller & Co.","facet":"work","description":"Reviewed terms."},{"name":"","facet":"work","description":"Invalid."},"invalid"]}"#,
            "20260101",
            "090000_300",
            None,
        )
        .unwrap();
        let rows =
            solstone_core_facets::read_detected_entities(root.path(), "work", "20260101").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Müller & Co.");
        let outcome: Value = serde_json::from_str(
            &fs::read_to_string(segment.join("detection_outcome.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["wrote"], 1);
        assert_eq!(outcome["dropped"], 2);
        assert_eq!(outcome["errored"], 0);
    }

    #[test]
    fn reconcile_errors_write_an_outcome_without_failing_the_stage() {
        // Derived from solstone/apps/entities/talent/detection.py:290-306.
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/090000_300/talents");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("sense.json"),
            r#"{"facets":[{"facet":"work"}],"entities":[{"name":"Ada","type":"Person"}]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.path().join("facets/work/entities/20260101.jsonl")).unwrap();
        apply_result(
            root.path(),
            r#"{"detections":[{"name":"Ada","facet":"work","description":"Discussed plans."}]}"#,
            "20260101",
            "090000_300",
            None,
        )
        .unwrap();
        let outcome: Value = serde_json::from_str(
            &fs::read_to_string(segment.join("detection_outcome.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["errored"], 1);
        assert!(
            outcome["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
    }

    #[test]
    fn gate_reports_each_reference_skip_path() {
        // Derived from solstone/apps/entities/talent/detection.py:189-214.
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let prepared = |config| PreparedTalent {
            name: "entities:detection".to_owned(),
            config,
        };
        assert!(
            matches!(gate(&prepared(Map::new()), &context).unwrap(), GateDecision::Skip(reason) if reason == "no_sense")
        );
        let segment = root.path().join("chronicle/20260101/090000_300/talents");
        fs::create_dir_all(&segment).unwrap();
        let config = Map::from_iter([
            ("day".to_owned(), Value::String("20260101".to_owned())),
            ("segment".to_owned(), Value::String("090000_300".to_owned())),
        ]);
        fs::write(segment.join("sense.json"), r#"{"facets":[],"entities":[]}"#).unwrap();
        assert!(
            matches!(gate(&prepared(config.clone()), &context).unwrap(), GateDecision::Skip(reason) if reason == "no_facets")
        );
        fs::write(
            segment.join("sense.json"),
            r#"{"facets":[{"facet":"work"}],"entities":[]}"#,
        )
        .unwrap();
        assert!(
            matches!(gate(&prepared(config), &context).unwrap(), GateDecision::Skip(reason) if reason == "no_candidates")
        );
    }
}
