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
    let mut attributed = BTreeSet::new();
    let mut named_speakers = Vec::new();
    for segment in segments {
        attributed.extend(attributed_entity_ids(journal, day, segment));
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
        let entry_name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let corroborated = entity_id.is_some_and(|id| attributed.contains(id))
            || name_resolves_to(
                &SpeakerEvidence {
                    journal,
                    entities: &entities,
                    facet,
                    day,
                    record_id,
                    names: &named_speakers,
                },
                entity_id,
                entry_name,
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
}

fn name_resolves_to(
    evidence: &SpeakerEvidence<'_>,
    entity_id: Option<&str>,
    entry_name: &str,
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
        if entity_id.is_some_and(|id| resolved_id(&result, evidence.entities).as_str() == Some(id))
        {
            return Ok(true);
        }
        if result.outcome == solstone_core_entity::EntityResolutionOutcome::NoMatch
            && !entry_name.is_empty()
            && name.to_lowercase() == entry_name.to_lowercase()
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

fn attributed_entity_ids(journal: &std::path::Path, day: &str, segment: &str) -> BTreeSet<String> {
    segment_talent_json(journal, day, segment, "speaker_labels.json")
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|label| label.get("speaker").and_then(Value::as_str))
        .filter(|speaker| !speaker.is_empty())
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
    use super::*;

    #[test]
    fn missing_segment_resolution_is_non_creating() {
        // Derived from solstone/talent/participation.py:24-30 and
        // solstone/think/cluster.py:749-764; Python is not runnable here.
        let root = tempfile::tempdir().unwrap();
        assert!(find_segment_dir(root.path(), "20260101", "090000_60", None).is_none());
        assert!(!root.path().join("chronicle/20260101").exists());
    }
}
