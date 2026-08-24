// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Participation post-hook.

use std::collections::BTreeSet;
use std::fs;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_system_health::find_segment_dir;

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{PreparedTalent, StageError, detected_resolution_entities, stage_error};

pub fn parse(
    output: &str,
    _prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "participation",
            prepared,
            "expected text output",
        ));
    };
    let Some(activity) = prepared.config.get("activity").and_then(Value::as_object) else {
        return Ok(CommitPlan::NoOutput);
    };
    let Some(facet) = prepared.config.get("facet").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    let Some(day) = prepared.config.get("day").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::Participation {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
        activity: activity.clone(),
    }))
}

pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    day: &str,
    activity: &Map<String, Value>,
) -> Result<(), String> {
    // Preserve solstone/talent/participation.py:76-105: malformed model output is ignored.
    let Ok(Value::Object(data)) = serde_json::from_str(output.trim()) else {
        return Ok(());
    };
    let Some(record_id) = activity
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(());
    };
    let Some(entries) = data.get("participation").and_then(Value::as_array) else {
        return Ok(());
    };
    let entities = detected_resolution_entities(journal, facet, day)?;
    let origin = |field: &str| json!({"lane":"talent.participation","facet":facet,"day":day,"record_id":record_id,"field":field});
    let mut resolved = Vec::new();
    for entry in entries.iter().filter_map(Value::as_object) {
        let mut entry = entry.clone();
        let result = solstone_core_entity::record_entity_resolution(
            journal,
            entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &entities,
            json!({"kind":"facet","facet":facet}),
            origin("participation.name"),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        entry.insert("entity_id".to_owned(), resolved_id(&result, &entities));
        resolved.push(entry);
    }
    let segments = activity
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if !segments.is_empty()
        && !segments
            .iter()
            .any(|segment| meeting_detected(journal, day, segment))
    {
        for entry in &mut resolved {
            if entry.get("role").and_then(Value::as_str) == Some("attendee") {
                entry.insert("role".to_owned(), Value::String("mentioned".to_owned()));
            }
        }
    }
    let admitted_entity_ids = solstone_core_entity::load_all_journal_entities(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(solstone_core_entity::is_admissible_person)
        .map(|entity| entity.id)
        .collect::<BTreeSet<_>>();
    let mut attributed = BTreeSet::new();
    let mut named_speakers = Vec::new();
    for segment in segments {
        attributed.extend(attributed_entity_ids(
            journal,
            day,
            segment,
            &admitted_entity_ids,
        ));
        named_speakers.extend(named_speakers_for_segment(journal, day, segment));
    }
    for entry in &mut resolved {
        if entry.get("role").and_then(Value::as_str) != Some("attendee")
            || !matches!(
                entry.get("source").and_then(Value::as_str),
                Some("voice" | "speaker_label")
            )
        {
            continue;
        }
        let entity_id = entry.get("entity_id").and_then(Value::as_str);
        let corroborated = entity_id.is_some_and(|id| attributed.contains(id))
            || name_resolves_to(
                &SpeakerEvidence {
                    journal,
                    entities: &entities,
                    facet,
                    day,
                    record_id,
                    names: &named_speakers,
                    admitted_entity_ids: &admitted_entity_ids,
                },
                entity_id,
            )?;
        if !corroborated {
            entry.insert("role".to_owned(), Value::String("mentioned".to_owned()));
        }
    }
    let mut patch = Map::from_iter([(
        "participation".to_owned(),
        Value::Array(resolved.into_iter().map(Value::Object).collect()),
    )]);
    if let Some(confidence) = data.get("participation_confidence") {
        patch.insert("participation_confidence".to_owned(), confidence.clone());
    }
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    solstone_core_facets::update_activity_record(
        journal,
        facet,
        day,
        record_id,
        &patch,
        "participation",
        "updated participation",
        &timestamp,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn resolved_id(
    result: &solstone_core_entity::EntityResolution,
    entities: &[solstone_core_entity::EntityResolutionEntity],
) -> Value {
    if result.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved {
        return result
            .entity_index
            .and_then(|index| entities[index].id.clone())
            .map_or(Value::Null, Value::String);
    }
    Value::Null
}

struct SpeakerEvidence<'a> {
    journal: &'a std::path::Path,
    entities: &'a [solstone_core_entity::EntityResolutionEntity],
    facet: &'a str,
    day: &'a str,
    record_id: &'a str,
    names: &'a [String],
    admitted_entity_ids: &'a BTreeSet<String>,
}

fn name_resolves_to(
    evidence: &SpeakerEvidence<'_>,
    entity_id: Option<&str>,
) -> Result<bool, String> {
    for name in evidence.names {
        let result = solstone_core_entity::record_entity_resolution(
            evidence.journal,
            name,
            evidence.entities,
            json!({"kind":"facet","facet":evidence.facet}),
            json!({"lane":"talent.participation","facet":evidence.facet,"day":evidence.day,"record_id":evidence.record_id,"field":"speaker.name"}),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        if result.outcome == solstone_core_entity::EntityResolutionOutcome::Ambiguous {
            continue;
        }
        let resolved_id = resolved_id(&result, evidence.entities);
        if entity_id.is_some_and(|id| resolved_id.as_str() == Some(id))
            && resolved_id
                .as_str()
                .is_some_and(|id| evidence.admitted_entity_ids.contains(id))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn meeting_detected(journal: &std::path::Path, day: &str, segment: &str) -> bool {
    segment_talent_json(journal, day, segment, "sense.json")
        .and_then(|value| value.get("meeting_detected").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn attributed_entity_ids(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    admitted_entity_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    segment_talent_json(journal, day, segment, "speaker_labels.json")
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|label| label.get("speaker").and_then(Value::as_str))
        .filter(|speaker| !speaker.is_empty())
        .filter(|speaker| admitted_entity_ids.contains(*speaker))
        .map(str::to_owned)
        .collect()
}

fn named_speakers_for_segment(journal: &std::path::Path, day: &str, segment: &str) -> Vec<String> {
    segment_talent_json(journal, day, segment, "speakers.json")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|speaker| !speaker.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

fn segment_talent_json(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    name: &str,
) -> Option<Value> {
    // `find_segment_dir` preserves cluster._find_segment_dir(..., create=False).
    let directory = find_segment_dir(journal, day, segment, None)?;
    serde_json::from_str(&fs::read_to_string(directory.join("talents").join(name)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::*;

    const DAY: &str = "20260101";
    const FACET: &str = "work";
    const SEGMENT: &str = "090000_60";

    fn activity() -> Map<String, Value> {
        json!({"id":"activity","segments":[SEGMENT]})
            .as_object()
            .expect("activity is an object")
            .clone()
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().expect("fixture path has a parent"))
            .expect("fixture parent");
        fs::write(path, value.to_string()).expect("fixture JSON");
    }

    fn write_entity(root: &Path, id: &str, name: &str, entity_type: &str) {
        write_json(
            &root.join(format!("entities/{id}/entity.json")),
            json!({"id":id,"name":name,"type":entity_type}),
        );
    }

    fn write_participation_fixture(root: &Path, labels: Value, speakers: Value) {
        write_entity(root, "person", "Ada Lovelace", "Person");
        write_entity(root, "tool", "Deploy Bot", "Tool");
        write_entity(root, "project", "Atlas", "Project");
        let detected = [
            json!({"id":"person","name":"Ada Lovelace","type":"Person"}),
            json!({"id":"tool","name":"Deploy Bot","type":"Tool"}),
            json!({"id":"project","name":"Atlas","type":"Project"}),
        ]
        .into_iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let detected_path = root.join(format!("facets/{FACET}/entities/{DAY}.jsonl"));
        fs::create_dir_all(detected_path.parent().expect("detected parent"))
            .expect("detected directory");
        fs::write(detected_path, format!("{detected}\n")).expect("detected entities");
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        fs::create_dir_all(activity_path.parent().expect("activity parent"))
            .expect("activity directory");
        let activity_line = json!({"id":"activity","segments":[SEGMENT]}).to_string();
        fs::write(activity_path, format!("{activity_line}\n")).expect("activity");
        let talents = root.join(format!("chronicle/{DAY}/{SEGMENT}/talents"));
        fs::create_dir_all(&talents).expect("talents directory");
        write_json(
            talents.join("sense.json").as_path(),
            json!({"meeting_detected":true}),
        );
        write_json(talents.join("speaker_labels.json").as_path(), labels);
        write_json(talents.join("speakers.json").as_path(), speakers);
    }

    fn apply(root: &Path, entry: Value) -> Value {
        apply_result(
            root,
            &json!({"participation":[entry]}).to_string(),
            FACET,
            DAY,
            &activity(),
        )
        .expect("participation applies");
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let activity: Value = serde_json::from_str(
            fs::read_to_string(activity_path)
                .expect("activity reads")
                .lines()
                .next()
                .expect("activity row"),
        )
        .expect("activity parses");
        activity["participation"][0].clone()
    }

    #[test]
    fn missing_segment_resolution_is_non_creating() {
        // Derived from solstone/talent/participation.py:24-30 and
        // solstone/think/cluster.py:749-764; Python is not runnable here.
        let root = tempfile::tempdir().unwrap();
        assert!(find_segment_dir(root.path(), "20260101", "090000_60", None).is_none());
        assert!(!root.path().join("chronicle/20260101").exists());
    }

    #[test]
    fn speaker_labeled_tool_demotes_to_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(
            root.path(),
            json!({"labels":[{"speaker":"tool"}]}),
            json!([]),
        );

        let entry = apply(
            root.path(),
            json!({"name":"Deploy Bot","role":"attendee","source":"speaker_label"}),
        );

        assert_eq!(entry["entity_id"], "tool");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn raw_named_project_demotes_to_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!(["Atlas"]));

        let entry = apply(
            root.path(),
            json!({"name":"Atlas","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], "project");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn speaker_labeled_person_remains_corroborated() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(
            root.path(),
            json!({"labels":[{"speaker":"person"}]}),
            json!([]),
        );

        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"speaker_label"}),
        );

        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["role"], "attendee");
    }

    #[test]
    fn non_speaker_tool_reference_keeps_generic_resolution() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));

        let entry = apply(
            root.path(),
            json!({"name":"Deploy Bot","role":"attendee","source":"manual"}),
        );

        assert_eq!(entry["entity_id"], "tool");
        assert_eq!(entry["role"], "attendee");
    }
}
