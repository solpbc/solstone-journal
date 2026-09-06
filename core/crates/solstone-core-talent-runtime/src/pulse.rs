// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cadence-pulse hook stages.

use std::collections::BTreeMap;
use std::fs;

use chrono::{Local, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_facets::{
    load_activity_records, load_current, load_imports, load_recent_entity_names,
};
use solstone_core_home::{
    HomeContext,
    readers::{collect_anticipated_activities, read_latest},
};
use solstone_core_system_health::find_segment_dir;

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

const MAX_UNITS: usize = 8;
const MAX_NEEDS: usize = 7;
const TITLE_MAX: usize = 80;
const SENTENCE_MAX: usize = 240;
const DETAILS_MAX: usize = 1800;
const NEED_MAX: usize = 240;
const PARTNER_MAX: usize = 4000;

#[derive(Clone, Debug, PartialEq)]
pub struct PulseSummary {
    title: String,
    one_sentence: String,
    full_details: String,
    needs_you: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PulseWindowNote {
    segments: usize,
    activities: usize,
    input_segments: usize,
    input_activities: usize,
    since_ms: Value,
    gaps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PulsePreState {
    default: PulseSummary,
    window: PulseWindowNote,
    previous_pulse: String,
    completed_since: String,
    awareness: String,
    anticipated: String,
    recent_entities: String,
    partner_profile: String,
    gaps: String,
}

pub fn gate(
    _prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    Ok(GateDecision::Proceed)
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    // Preserve solstone/talent/pulse.py:284-333: a failed source packet skips generation.
    build_packet(prepared, context)
        .map(|state| PrePostState::Pulse(Box::new(state)))
        .map_err(|detail| RuntimeOutcome::Skipped {
            stage: "pulse".to_owned(),
            talent: prepared.name.clone(),
            reason: format!("pulse pre-hook failed: {detail}"),
        })
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::Pulse(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "pulse",
            prepared,
            "missing pulse state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([
            (
                "previous_pulse".to_owned(),
                Value::String(state.previous_pulse.clone()),
            ),
            (
                "completed_since".to_owned(),
                Value::String(state.completed_since.clone()),
            ),
            (
                "awareness".to_owned(),
                Value::String(state.awareness.clone()),
            ),
            (
                "anticipated".to_owned(),
                Value::String(state.anticipated.clone()),
            ),
            (
                "recent_entities".to_owned(),
                Value::String(state.recent_entities.clone()),
            ),
            (
                "partner_profile".to_owned(),
                Value::String(state.partner_profile.clone()),
            ),
            ("gaps".to_owned(), Value::String(state.gaps.clone())),
        ]),
    );
    Ok(())
}

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
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let PrePostState::Pulse(state) = state else {
        return Err(stage_error(
            "commit",
            "pulse",
            prepared,
            "missing pulse state",
        ));
    };
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "pulse",
            prepared,
            "expected text output",
        ));
    };
    let summary = normalize_pulse(&output, &state.default);
    let day = configured_day(prepared);
    let mut record = summary_value(&summary).as_object().cloned().unwrap();
    record.insert(
        "model".to_owned(),
        prepared.config.get("model").cloned().unwrap_or(Value::Null),
    );
    record.insert(
        "generated_at".to_owned(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)),
    );
    record.insert("ts".to_owned(), Value::from(Utc::now().timestamp_millis()));
    record.insert("window".to_owned(), window_value(&state.window));
    Ok(CommitPlan::Write(WriteIntent::DayAccumulator {
        day,
        agent: "pulse".to_owned(),
        record,
    }))
}

pub fn output_override(
    output: &str,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<String, StageError> {
    let PrePostState::Pulse(state) = state else {
        return Err(stage_error(
            "output_override",
            "pulse",
            prepared,
            "missing pulse state",
        ));
    };
    // Preserve solstone/talent/pulse.py:393-423: normalization is shared with the write.
    serde_json::to_string_pretty(&summary_value(&normalize_pulse(output, &state.default)))
        .map_err(|error| stage_error("output_override", "pulse", prepared, error.to_string()))
}

fn build_packet(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<PulsePreState, String> {
    let day = configured_day(prepared);
    let now = Utc::now();
    let home = HomeContext::new(&context.journal, now);
    let mut gaps = Vec::new();
    let default = default_pulse();
    let previous = read_latest(&home, &day, "pulse", 7);
    let (completed, window) = completed_since(&prepared.config, context, &mut gaps);
    let awareness = awareness_context(&context.journal, &mut gaps);
    // This reader already owns the declared-facet activity scan used by the reference.
    let anticipated = collect_anticipated_activities(&home, &day);
    let recent = match load_recent_entity_names(&context.journal, 12) {
        Ok(Some(names)) => names,
        Ok(None) => Vec::new(),
        Err(error) => {
            gaps.push(format!("could not read recent entities: {error}"));
            Vec::new()
        }
    };
    let partner = read_partner_profile(&context.journal, &mut gaps);
    Ok(PulsePreState {
        default,
        window,
        previous_pulse: previous.map_or_else(|| "(none - first run)".to_owned(), compact_json),
        completed_since: compact_json(completed),
        awareness: compact_json(awareness),
        anticipated: compact_json(Value::Array(anticipated)),
        recent_entities: compact_json(json!(recent)),
        partner_profile: partner,
        gaps: if gaps.is_empty() {
            "(none)".to_owned()
        } else {
            gaps.iter()
                .map(|gap| format!("- {gap}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
    })
}

fn configured_day(prepared: &PreparedTalent) -> String {
    prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Local::now().format("%Y%m%d").to_string())
}

fn completed_since(
    config: &Map<String, Value>,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> (Value, PulseWindowNote) {
    let window = config.get("cadence_window").and_then(Value::as_object);
    let segment_units = window
        .and_then(|value| value.get("segments"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let activity_units = window
        .and_then(|value| value.get("activities"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let since_ms = window
        .and_then(|value| value.get("since_ms"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut units = segment_units
        .iter()
        .filter_map(|unit| {
            unit.as_object()
                .cloned()
                .map(|unit| (unit_timestamp(&unit), "segment", unit))
        })
        .collect::<Vec<_>>();
    units.extend(activity_units.iter().filter_map(|unit| {
        unit.as_object()
            .cloned()
            .map(|unit| (unit_timestamp(&unit), "activity", unit))
    }));
    units.sort_by_key(|(ts, _, _)| std::cmp::Reverse(*ts));
    let mut segments = Vec::new();
    let mut activities = Vec::new();
    for (_, kind, unit) in units.into_iter().take(MAX_UNITS) {
        let Some(day) = unit.get("day").and_then(Value::as_str) else {
            gaps.push("completed unit missing source day".to_owned());
            continue;
        };
        if kind == "segment" {
            if let Some(value) = read_segment_activity(day, &unit, context, gaps) {
                segments.push(value);
            }
        } else if let Some(value) = read_activity(day, &unit, context, gaps) {
            activities.push(value);
        }
    }
    let note = PulseWindowNote {
        segments: segments.len(),
        activities: activities.len(),
        input_segments: segment_units.len(),
        input_activities: activity_units.len(),
        since_ms: since_ms.clone(),
        gaps: gaps.clone(),
    };
    (
        json!({"since_ms": since_ms, "input_segments": segment_units.len(), "input_activities": activity_units.len(), "segments": segments, "activities": activities}),
        note,
    )
}

fn unit_timestamp(unit: &Map<String, Value>) -> i64 {
    unit.get("ts")
        .and_then(Value::as_i64)
        .or_else(|| {
            unit.get("ts")
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

fn read_segment_activity(
    day: &str,
    unit: &Map<String, Value>,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> Option<Value> {
    let segment = string_or(unit.get("segment"), "");
    if segment.is_empty() {
        gaps.push("completed segment missing segment id".to_owned());
        return None;
    }
    let stream = unit
        .get("stream")
        .map(|value| string_or(Some(value), ""))
        .filter(|value| !value.is_empty());
    let Some(segment_dir) = find_segment_dir(&context.journal, day, &segment, stream.as_deref())
    else {
        gaps.push(format!("could not find completed segment {segment}"));
        return None;
    };
    let activity = match fs::read_to_string(segment_dir.join("talents/activity.md")) {
        Ok(text) if !text.trim().is_empty() => text.trim().to_owned(),
        _ => {
            gaps.push(format!("no activity summary for segment {segment}"));
            return None;
        }
    };
    Some(
        json!({"day": day, "segment": segment, "stream": stream, "ts": unit.get("ts").cloned().unwrap_or(Value::Null), "activity": activity}),
    )
}

fn read_activity(
    day: &str,
    unit: &Map<String, Value>,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> Option<Value> {
    let facet = string_or(unit.get("facet"), "");
    let activity = string_or(unit.get("activity"), "");
    if facet.is_empty() || activity.is_empty() {
        gaps.push(format!(
            "completed activity missing facet or id: {}",
            if activity.is_empty() {
                "(none)"
            } else {
                &activity
            }
        ));
        return None;
    }
    let records = match load_activity_records(&context.journal, &facet, day, true) {
        Ok(records) => records,
        Err(error) => {
            gaps.push(format!("could not load activities for {facet}: {error}"));
            return None;
        }
    };
    let Some(record) = records
        .into_iter()
        .find(|record| string_or(record.get("id"), "") == activity)
    else {
        gaps.push(format!("activity record not found: {facet}/{activity}"));
        return None;
    };
    Some(
        json!({"day": day, "facet": facet, "activity": activity, "ts": unit.get("ts").cloned().unwrap_or(Value::Null), "title": string_or(record.get("title"), &activity.replace('_', " ")), "description": string_or(record.get("description"), ""), "details": string_or(record.get("details"), ""), "source": record.get("source").cloned().unwrap_or(Value::Null), "segments": record.get("segments").cloned().unwrap_or_else(|| json!([]))}),
    )
}

fn awareness_context(journal: &std::path::Path, gaps: &mut Vec<String>) -> Value {
    let current = load_current(journal).unwrap_or_else(|error| {
        gaps.push(format!("could not read current awareness: {error}"));
        json!({})
    });
    let imports = load_imports(journal).unwrap_or_else(|error| {
        gaps.push(format!("could not read import awareness: {error}"));
        json!({})
    });
    json!({"current": current, "imports": imports})
}

fn read_partner_profile(journal: &std::path::Path, gaps: &mut Vec<String>) -> String {
    match fs::read_to_string(journal.join("identity/partner.md")) {
        Ok(value) if !value.trim().is_empty() => truncate(value.trim(), PARTNER_MAX),
        Ok(_) => "(empty)".to_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            gaps.push("identity/partner.md missing".to_owned());
            "(missing)".to_owned()
        }
        Err(error) => {
            gaps.push(format!("could not read identity/partner.md: {error}"));
            "(unavailable)".to_owned()
        }
    }
}

fn default_pulse() -> PulseSummary {
    PulseSummary { title: "Day in progress".to_owned(), one_sentence: "The day is still taking shape.".to_owned(), full_details: "There is not enough current context to name a clear shape yet. Sol will keep watching for completed segments, anticipated events, and anything that needs the owner's attention.".to_owned(), needs_you: Vec::new() }
}

fn normalize_pulse(raw: &str, default: &PulseSummary) -> PulseSummary {
    coerce_pulse(raw).unwrap_or_else(|| PulseSummary {
        title: truncate(
            &string_or(
                Some(&Value::String(default.title.clone())),
                "Day in progress",
            ),
            TITLE_MAX,
        ),
        one_sentence: truncate(
            &string_or(
                Some(&Value::String(default.one_sentence.clone())),
                "The day is still taking shape.",
            ),
            SENTENCE_MAX,
        ),
        full_details: truncate(
            &string_or(
                Some(&Value::String(default.full_details.clone())),
                "There is not enough current context to name a clear shape yet.",
            ),
            DETAILS_MAX,
        ),
        needs_you: coerce_needs(&Value::Array(
            default
                .needs_you
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        )),
    })
}

fn coerce_pulse(raw: &str) -> Option<PulseSummary> {
    let data = serde_json::from_str::<Value>(raw).ok().or_else(|| {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        (end > start)
            .then(|| serde_json::from_str(&raw[start..=end]).ok())
            .flatten()
    })?;
    let object = data.as_object()?;
    let title = object.get("title")?.as_str()?.trim();
    let one_sentence = object.get("one_sentence")?.as_str()?.trim();
    let full_details = object.get("full_details")?.as_str()?.trim();
    (!title.is_empty() && !one_sentence.is_empty() && !full_details.is_empty()).then(|| {
        PulseSummary {
            title: truncate(title, TITLE_MAX),
            one_sentence: truncate(one_sentence, SENTENCE_MAX),
            full_details: truncate(full_details, DETAILS_MAX),
            needs_you: coerce_needs(object.get("needs_you").unwrap_or(&Value::Null)),
        }
    })
}

fn coerce_needs(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| !value.is_null())
        .map(python_string)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(|value| truncate(&value, NEED_MAX))
        .take(MAX_NEEDS)
        .collect()
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
fn string_or(value: Option<&Value>, fallback: &str) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}
fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
fn compact_json(value: Value) -> String {
    serde_json::to_string_pretty(&sort_value(value)).unwrap_or_else(|_| "null".to_owned())
}
fn sort_value(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, sort_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(sort_value).collect()),
        value => value,
    }
}
fn summary_value(summary: &PulseSummary) -> Value {
    json!({"full_details": summary.full_details, "needs_you": summary.needs_you, "one_sentence": summary.one_sentence, "title": summary.title})
}
fn window_value(window: &PulseWindowNote) -> Value {
    json!({"segments": window.segments, "activities": window.activities, "input_segments": window.input_segments, "input_activities": window.input_activities, "since_ms": window.since_ms, "gaps": window.gaps})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fenced_output_and_uses_reference_defaults() {
        // Derived from solstone/talent/pulse.py:339-423; Python is not runnable here.
        let summary = normalize_pulse(
            "text {\"title\":\"  A  \",\"one_sentence\":\"B\",\"full_details\":\"C\",\"needs_you\":[\" x \",null]} tail",
            &default_pulse(),
        );
        assert_eq!(summary.title, "A");
        assert_eq!(summary.needs_you, vec!["x"]);
        assert_eq!(
            normalize_pulse("not json", &default_pulse()).title,
            "Day in progress"
        );
    }

    #[test]
    fn output_override_is_sorted_pretty_json() {
        // Derived from solstone/talent/pulse.py:423; Python is not runnable here.
        let state = PrePostState::Pulse(Box::new(PulsePreState {
            default: default_pulse(),
            window: PulseWindowNote {
                segments: 0,
                activities: 0,
                input_segments: 0,
                input_activities: 0,
                since_ms: Value::Null,
                gaps: Vec::new(),
            },
            previous_pulse: String::new(),
            completed_since: String::new(),
            awareness: String::new(),
            anticipated: String::new(),
            recent_entities: String::new(),
            partner_profile: String::new(),
            gaps: String::new(),
        }));
        let prepared = PreparedTalent {
            name: "pulse".to_owned(),
            config: Map::new(),
        };
        assert_eq!(
            output_override(
                r#"{"title":"T","one_sentence":"S","full_details":"D","needs_you":["N"]}"#,
                &prepared,
                &state,
            )
            .unwrap(),
            "{\n  \"full_details\": \"D\",\n  \"needs_you\": [\n    \"N\"\n  ],\n  \"one_sentence\": \"S\",\n  \"title\": \"T\"\n}"
        );
    }
}
