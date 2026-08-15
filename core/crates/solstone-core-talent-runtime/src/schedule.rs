// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Schedule post-hook.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{PreparedTalent, StageError, detected_resolution_entities, stage_error};

const ANTICIPATION_FUZZY_THRESHOLD: f64 = 0.85;

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
            "schedule",
            prepared,
            "expected text output",
        ));
    };
    let Some(day) = prepared.config.get("day").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::Schedule {
        output,
        day: day.to_owned(),
    }))
}

pub fn apply_result(journal: &std::path::Path, output: &str, day: &str) -> Result<(), String> {
    // Preserve solstone/talent/schedule.py:45-71: malformed output is ignored.
    let Ok(mut events) = serde_json::from_str::<Value>(output.trim()) else {
        return Ok(());
    };
    if events.is_object() {
        events = events
            .as_object_mut()
            .and_then(|object| object.remove("events"))
            .unwrap_or_else(|| Value::Array(Vec::new()));
    }
    let Some(events) = events.as_array() else {
        return Ok(());
    };
    let Ok(current_day) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return Ok(());
    };
    let known_facets = solstone_core_facets::list_declared_facet_names(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut entity_cache = BTreeMap::new();
    for raw in events {
        let Some(raw) = raw.as_object() else { continue };
        let _ = apply_event(
            journal,
            raw,
            day,
            current_day,
            &known_facets,
            &mut entity_cache,
        );
        // Python skips one invalid event and continues with the following event.
    }
    Ok(())
}

fn apply_event(
    journal: &std::path::Path,
    raw: &Map<String, Value>,
    day: &str,
    current_day: NaiveDate,
    known_facets: &BTreeSet<String>,
    entity_cache: &mut BTreeMap<
        (String, String),
        Vec<solstone_core_entity::EntityResolutionEntity>,
    >,
) -> Result<(), String> {
    let activity = require_text(raw, "activity")?;
    let target_date = require_text(raw, "target_date")?;
    let title = require_text(raw, "title")?;
    let description = require_text(raw, "description")?;
    let facet = require_text(raw, "facet")?;
    if !known_facets.contains(&facet) {
        return Err(format!("unknown facet {facet:?}"));
    }
    let target_day =
        NaiveDate::parse_from_str(&target_date, "%Y-%m-%d").map_err(|error| error.to_string())?;
    if target_day <= current_day {
        return Err(format!(
            "target_date must be after context day ({target_date} <= {day})"
        ));
    }
    let start = optional_time(raw, "start")?;
    let end = optional_time(raw, "end")?;
    let cancelled = raw
        .get("cancelled")
        .is_some_and(solstone_core_facets::activity_value_truthy);
    let details = python_string(raw.get("details").unwrap_or(&Value::Null));
    let participation_confidence = raw
        .get("participation_confidence")
        .cloned()
        .unwrap_or(Value::Null);
    let Some(participation) = raw.get("participation").and_then(Value::as_array) else {
        return Err("participation must be a list".to_owned());
    };
    let target_day_key = target_day.format("%Y%m%d").to_string();
    let entities = entity_cache
        .entry((facet.clone(), target_day_key.clone()))
        .or_insert(detected_resolution_entities(
            journal,
            &facet,
            &target_day_key,
        )?);
    let new_id = make_anticipation_id(&activity, start.as_deref(), &target_date)?;
    let mut resolved = Vec::new();
    let mut active = Vec::new();
    let mut seen_active = BTreeSet::new();
    for entry in participation.iter().filter_map(Value::as_object) {
        let mut entry = entry.clone();
        let resolution = solstone_core_entity::record_entity_resolution(
            journal,
            entry.get("name").and_then(Value::as_str).unwrap_or_default(),
            entities,
            json!({"kind":"facet","facet":facet}),
            json!({"lane":"talent.schedule","facet":facet,"day":target_day_key,"record_id":new_id,"field":"participation.name"}),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        let entity_id = resolved_id(&resolution, entities);
        entry.insert("entity_id".to_owned(), entity_id.clone());
        if entry.get("role").and_then(Value::as_str) == Some("attendee")
            && let Some(id) = entity_id.as_str()
            && seen_active.insert(id.to_owned())
        {
            active.push(Value::String(id.to_owned()));
        }
        resolved.push(Value::Object(entry));
    }
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let fields = [
        "activity",
        "target_date",
        "start",
        "end",
        "title",
        "description",
        "details",
        "source",
        "active_entities",
        "participation",
        "participation_confidence",
        "cancelled",
        "hidden",
    ];
    let record = solstone_core_facets::append_edit(
        Map::from_iter([
            ("id".to_owned(), Value::String(new_id.clone())),
            ("activity".to_owned(), Value::String(activity)),
            ("target_date".to_owned(), Value::String(target_date)),
            ("start".to_owned(), start.map_or(Value::Null, Value::String)),
            ("end".to_owned(), end.map_or(Value::Null, Value::String)),
            ("title".to_owned(), Value::String(title)),
            ("description".to_owned(), Value::String(description)),
            ("details".to_owned(), Value::String(details)),
            ("facet".to_owned(), Value::String(facet.clone())),
            ("source".to_owned(), Value::String("anticipated".to_owned())),
            ("active_entities".to_owned(), Value::Array(active)),
            ("participation".to_owned(), Value::Array(resolved)),
            (
                "participation_confidence".to_owned(),
                participation_confidence,
            ),
            ("cancelled".to_owned(), Value::Bool(cancelled)),
            ("hidden".to_owned(), Value::Bool(cancelled)),
        ]),
        "schedule",
        fields.into_iter().map(str::to_owned).collect(),
        if cancelled {
            "created by schedule (cancelled on calendar)"
        } else {
            "created by schedule"
        },
        &timestamp,
    );
    let (write, superseded) = dedup_anticipation(journal, &facet, &target_day_key, &record)?;
    if !write {
        return Ok(());
    }
    if !matches!(
        solstone_core_facets::append_activity_record(journal, &facet, &target_day_key, record)
            .map_err(|error| error.to_string())?,
        solstone_core_facets::AppendOutcome::Written(_)
    ) {
        return Ok(());
    }
    for superseded_id in superseded {
        solstone_core_facets::set_activity_hidden(
            journal,
            &facet,
            &target_day_key,
            &superseded_id,
            true,
            "schedule",
            Some(&format!("superseded by {new_id}")),
            &timestamp,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn require_text(item: &Map<String, Value>, key: &str) -> Result<String, String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing required field '{key}'"))
}

fn optional_time(item: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    let Some(value) = item.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(format!("invalid {key:?}: expected HH:MM:SS or null"));
    };
    Ok(Some(
        (value.len() == 8
            && value.as_bytes()[2] == b':'
            && value.as_bytes()[5] == b':'
            && value
                .bytes()
                .enumerate()
                .all(|(index, byte)| matches!(index, 2 | 5) || byte.is_ascii_digit()))
        .then_some(value.to_owned())
        .ok_or_else(|| format!("invalid {key:?}: expected HH:MM:SS or null"))?,
    ))
}

// This is a writer's key derived from name-and-position. Preserve the inherited
// reference behavior from solstone/think/activities.py:1350; do not treat it as a stable identity rule.
fn make_anticipation_id(
    activity_type: &str,
    start: Option<&str>,
    target_date: &str,
) -> Result<String, String> {
    let activity = activity_type.trim();
    if activity.is_empty() {
        return Err("activity_type must be non-empty".to_owned());
    }
    let target = NaiveDate::parse_from_str(target_date, "%Y-%m-%d")
        .map_err(|_| "target_date must match YYYY-MM-DD".to_owned())?;
    let start = match start {
        Some(start) => optional_time(
            &Map::from_iter([("start".to_owned(), Value::String(start.to_owned()))]),
            "start",
        )?
        .unwrap()
        .replace(':', ""),
        None => "000000".to_owned(),
    };
    Ok(format!(
        "anticipated_{activity}_{start}_{:02}{:02}",
        target.month(),
        target.day()
    ))
}

fn dedup_anticipation(
    journal: &std::path::Path,
    facet: &str,
    target_day: &str,
    record: &Map<String, Value>,
) -> Result<(bool, Vec<String>), String> {
    let new_id = require_text(record, "id")?;
    let new_title = normalized_title(record.get("title"));
    let mut superseded = Vec::new();
    for existing in solstone_core_facets::load_activity_records(journal, facet, target_day, false)
        .map_err(|error| error.to_string())?
    {
        if existing.get("source").and_then(Value::as_str) != Some("anticipated") {
            continue;
        }
        let existing_id = python_string(existing.get("id").unwrap_or(&Value::Null))
            .trim()
            .to_owned();
        if existing_id == new_id {
            return Ok((false, Vec::new()));
        }
        if sequence_ratio(&new_title, &normalized_title(existing.get("title")))
            >= ANTICIPATION_FUZZY_THRESHOLD
        {
            superseded.push(existing_id);
        }
    }
    Ok((true, superseded))
}

fn resolved_id(
    result: &solstone_core_entity::EntityResolution,
    entities: &[solstone_core_entity::EntityResolutionEntity],
) -> Value {
    if result.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved {
        result
            .entity_index
            .and_then(|index| entities[index].id.clone())
            .map_or(Value::Null, Value::String)
    } else {
        Value::Null
    }
}
fn normalized_title(value: Option<&Value>) -> String {
    python_string(value.unwrap_or(&Value::Null))
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn python_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => {
            if *value {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Null => "None".into(),
        value => value.to_string(),
    }
}

/// Python `difflib.SequenceMatcher(None, a, b).ratio()` over Unicode code points.
fn sequence_ratio(left: &str, right: &str) -> f64 {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    let total = left.len() + right.len();
    if total == 0 {
        return 1.0;
    }
    let mut queue = vec![(0, left.len(), 0, right.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, size) = longest_match(&left, &right, alo, ahi, blo, bhi);
        if size == 0 {
            continue;
        }
        blocks.push((i, j, size));
        if alo < i && blo < j {
            queue.push((alo, i, blo, j));
        }
        if i + size < ahi && j + size < bhi {
            queue.push((i + size, ahi, j + size, bhi));
        }
    }
    blocks.sort_unstable();
    let mut matched = 0;
    let mut prior = (0, 0, 0);
    for block in blocks {
        if prior.0 + prior.2 == block.0 && prior.1 + prior.2 == block.1 {
            prior.2 += block.2;
        } else {
            matched += prior.2;
            prior = block;
        }
    }
    matched += prior.2;
    2.0 * matched as f64 / total as f64
}

fn longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let mut best = (alo, blo, 0);
    let mut previous = BTreeMap::new();
    for (i, left) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut current = BTreeMap::new();
        for (j, right) in b.iter().enumerate().take(bhi).skip(blo) {
            if left != right {
                continue;
            }
            let size = previous.get(&j.saturating_sub(1)).copied().unwrap_or(0) + 1;
            current.insert(j, size);
            if size > best.2 {
                best = (i + 1 - size, j + 1 - size, size);
            }
        }
        previous = current;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anticipation_id_and_fuzzy_supersede_match_reference_shape() {
        // Derived from solstone/think/activities.py:1350-1401.
        assert_eq!(
            make_anticipation_id("meeting", Some("09:30:00"), "2026-03-14").unwrap(),
            "anticipated_meeting_093000_0314"
        );
        assert!(sequence_ratio("project sync", "project synch") >= ANTICIPATION_FUZZY_THRESHOLD);
    }
}
