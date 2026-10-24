// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Morning-briefing pre-hook.

use chrono::{NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_facets::{load_activity_records, read_facet_declaration};
use solstone_core_home::{
    HomeContext,
    briefing::BriefingDates,
    readers::{enabled_facet_names, read_latest},
};

use crate::contract::{GateDecision, PrePostState};
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DigestActivity {
    pub facet: String,
    pub day: String,
    pub id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub target_date: Option<String>,
    pub duration: Option<Value>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub details: Option<String>,
    /// Who attended. People an activity only mentions are left out, so the
    /// briefing is never told they were there.
    pub participants: Vec<String>,
    pub entities: Vec<String>,
    pub source_limitation: bool,
    pub story_body: Option<String>,
    /// What the owner's calendar had scheduled while the activity ran.
    #[serde(default)]
    pub scheduled: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MorningBriefingPreState {
    pub values: Map<String, Value>,
    pub activities: Vec<DigestActivity>,
    pub initial_gaps: Vec<String>,
    pub counts: Value,
    pub generated: String,
    pub model: String,
}

pub fn gate(
    prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let day = configured_day(prepared);
    if day.is_empty() {
        return Ok(GateDecision::Skip("missing day".to_owned()));
    }
    if NaiveDate::parse_from_str(&day, "%Y%m%d").is_err() {
        return Ok(GateDecision::Skip(format!("invalid day: {day}")));
    }
    Ok(GateDecision::Proceed)
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let day = configured_day(prepared);
    let analysis_day =
        NaiveDate::parse_from_str(&day, "%Y%m%d").map_err(|error| RuntimeOutcome::Skipped {
            stage: "morning_briefing".into(),
            talent: prepared.name.clone(),
            reason: format!("invalid day: {error}"),
        })?;
    let state = build_briefing_state(
        &day,
        analysis_day,
        prepared
            .config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        context,
    )
    .map_err(|error| RuntimeOutcome::Skipped {
        stage: "morning_briefing".into(),
        talent: prepared.name.clone(),
        reason: format!("morning briefing pre-hook failed: {error}"),
    })?;
    Ok(PrePostState::MorningBriefing(state))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::MorningBriefing(briefing_state) = state else {
        return Err(stage_error(
            "prompt_override",
            "morning_briefing",
            prepared,
            "missing morning briefing state",
        ));
    };

    let max_output_tokens = prepared
        .config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .unwrap_or(0);
    let floor_budget = solstone_core_local::generate::compute_input_budget(
        max_output_tokens,
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    );
    let schema_tokens = match prepared.config.get("json_schema") {
        Some(Value::String(s)) => solstone_core_local::generate::estimate_tokens(s),
        Some(val) => solstone_core_local::generate::estimate_tokens(&val.to_string()),
        None => 0,
    };

    let n = briefing_state.activities.len();
    let mut include_stories = vec![true; n];
    let mut include_activities = vec![true; n];

    let measure = |inc_acts: &[bool], inc_stories: &[bool]| -> (Map<String, Value>, u32) {
        let mut current_gaps = briefing_state.initial_gaps.clone();
        current_gaps.extend(omission_gaps(
            &briefing_state.activities,
            inc_acts,
            inc_stories,
        ));

        let mut values = briefing_state.values.clone();
        let digest_text =
            render_digest_activities(&briefing_state.activities, inc_acts, inc_stories);
        let metadata = json!({
            "generated": briefing_state.generated,
            "model": briefing_state.model,
            "sources": briefing_state.counts,
            "gaps": current_gaps,
            "coverage_preamble": coverage_preamble(&current_gaps),
        });
        values.insert("activity_digest".into(), Value::String(digest_text));
        values.insert(
            "briefing_metadata".into(),
            Value::String(serde_json::to_string_pretty(&metadata).unwrap_or_default()),
        );

        let mut config_clone = prepared.config.clone();
        apply_template_vars(&mut config_clone, &values);

        let text_tokens: u32 = crate::model_text_parts(&config_clone)
            .iter()
            .map(|part| solstone_core_local::generate::estimate_tokens(part))
            .sum();
        let total_tokens = text_tokens + schema_tokens;

        (values, total_tokens)
    };

    let (mut current_values, mut current_tokens) = measure(&include_activities, &include_stories);

    if current_tokens > floor_budget {
        for i in (0..n).rev() {
            if briefing_state.activities[i].story_body.is_some() {
                include_stories[i] = false;
                let (vals, tokens) = measure(&include_activities, &include_stories);
                current_values = vals;
                current_tokens = tokens;
                if current_tokens <= floor_budget {
                    break;
                }
            }
        }
    }

    if current_tokens > floor_budget {
        for i in (0..n).rev() {
            include_activities[i] = false;
            let (vals, tokens) = measure(&include_activities, &include_stories);
            current_values = vals;
            current_tokens = tokens;
            if current_tokens <= floor_budget {
                break;
            }
        }
    }

    if current_tokens > floor_budget {
        return Err(stage_error(
            "prompt_override",
            "morning_briefing",
            prepared,
            "prepared request does not fit the floor input budget",
        ));
    }

    if let Some(meta) = current_values.get("briefing_metadata").cloned() {
        prepared.config.insert("briefing_metadata".into(), meta);
    }
    let inclusion = briefing_state
        .activities
        .iter()
        .enumerate()
        .map(|(i, act)| {
            json!({
                "facet": act.facet,
                "day": act.day,
                "id": act.id,
                "activity": include_activities[i],
                "story": include_stories[i],
            })
        })
        .collect::<Vec<_>>();
    prepared
        .config
        .insert("_briefing_inclusion".into(), Value::Array(inclusion));
    apply_template_vars(&mut prepared.config, &current_values);
    Ok(())
}

fn configured_day(prepared: &PreparedTalent) -> String {
    prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

#[cfg(test)]
fn build_packet(
    day: &str,
    analysis_day: NaiveDate,
    model: &str,
    context: &ExecutionContext,
) -> Result<Map<String, Value>, String> {
    let state = build_briefing_state(day, analysis_day, model, context)?;
    Ok(state.values)
}

fn build_briefing_state(
    day: &str,
    analysis_day: NaiveDate,
    model: &str,
    context: &ExecutionContext,
) -> Result<MorningBriefingPreState, String> {
    let dates = BriefingDates::for_analysis(analysis_day).ok_or("briefing date overflow")?;
    let presentation_day = dates.presentation.format("%Y%m%d").to_string();
    let mut gaps = Vec::new();
    let home = HomeContext::new(&context.journal, Utc::now());
    let facets = enabled_facet_names(&home)
        .into_iter()
        .map(|name| {
            let title = read_facet_declaration(&context.journal, &name)
                .ok()
                .flatten()
                .map_or_else(|| name.clone(), |declaration| declaration.title);
            (name, title)
        })
        .collect::<Vec<_>>();
    if facets.is_empty() {
        gaps.push("active facets".to_owned());
    }

    let digest_activities = load_digest_activities(&facets, day, context, &mut gaps);

    let pulse = read_pulse(&home, day, &mut gaps);
    let counts = json!({"segments": 0});
    let generated = generated_stamp(&home);
    let metadata = json!({
        "generated": generated,
        "model": model,
        "sources": counts,
        "gaps": gaps,
        "coverage_preamble": coverage_preamble(&gaps),
    });
    let n = digest_activities.len();
    let include_activities = vec![true; n];
    let include_stories = vec![true; n];
    let values = Map::from_iter([
        (
            "briefing_analysis_day".into(),
            Value::String(day.to_owned()),
        ),
        (
            "briefing_presentation_day".into(),
            Value::String(presentation_day),
        ),
        (
            "briefing_metadata".into(),
            Value::String(
                serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?,
            ),
        ),
        (
            "active_facets".into(),
            Value::String(render_facets(&facets)),
        ),
        (
            "activity_digest".into(),
            Value::String(render_digest_activities(
                &digest_activities,
                &include_activities,
                &include_stories,
            )),
        ),
        (
            "pulse_surface".into(),
            Value::String(if pulse.is_empty() {
                "(missing)".into()
            } else {
                pulse
            }),
        ),
    ]);
    Ok(MorningBriefingPreState {
        values,
        activities: digest_activities,
        initial_gaps: gaps,
        counts,
        generated,
        model: model.to_owned(),
    })
}

fn load_digest_activities(
    facets: &[(String, String)],
    day: &str,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> Vec<DigestActivity> {
    let mut activities = Vec::new();
    let calendar = solstone_core_facets::calendar_on_day(&context.journal, day);
    for (facet_idx, (facet, title)) in facets.iter().enumerate() {
        match load_activity_records(&context.journal, facet, day, false) {
            Ok(records) => {
                let activity_path = context
                    .journal
                    .join("facets")
                    .join(facet)
                    .join("activities")
                    .join(format!("{day}.jsonl"));
                if activity_path.is_file()
                    && let Ok(content) = std::fs::read_to_string(&activity_path)
                {
                    let has_unreadable_line = content
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .any(|line| {
                            serde_json::from_str::<Value>(line)
                                .ok()
                                .and_then(|v| v.as_object().cloned())
                                .is_none()
                        });
                    if has_unreadable_line {
                        gaps.push(format!(
                            "activities from {title} include a line that could not be read"
                        ));
                    }
                }
                let mut has_unusable_id = false;
                for record in records {
                    let id = match record
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                    {
                        Some(id) => id.to_owned(),
                        None => {
                            has_unusable_id = true;
                            continue;
                        }
                    };
                    let title = record
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let description = record
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let details = record
                        .get("details")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let start = record
                        .get("start")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let end = record
                        .get("end")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let target_date = record
                        .get("target_date")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let duration = record.get("duration").cloned().filter(|v| !v.is_null());
                    let participants = attendee_names(&record);
                    let entities = record
                        .get("active_entities")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let story_body = record
                        .get("story")
                        .and_then(|s| s.get("body"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let source_limitation = record
                        .get("story")
                        .and_then(Value::as_object)
                        .is_some_and(|story| {
                            story
                                .get("body")
                                .and_then(Value::as_str)
                                .is_some_and(|b| !b.is_empty())
                                && story
                                    .get("partial_input")
                                    .and_then(Value::as_object)
                                    .is_some()
                        });

                    let scheduled = solstone_core_facets::activity_span(&record)
                        .map(|span| {
                            solstone_core_facets::calendar_beside(&calendar, day, span)
                                .iter()
                                .map(describe_scheduled)
                                .collect()
                        })
                        .unwrap_or_default();

                    activities.push((
                        facet_idx,
                        DigestActivity {
                            facet: facet.clone(),
                            day: day.to_owned(),
                            id,
                            start,
                            end,
                            target_date,
                            duration,
                            title,
                            description,
                            details,
                            participants,
                            entities,
                            source_limitation,
                            story_body,
                            scheduled,
                        },
                    ));
                }
                if has_unusable_id {
                    gaps.push(format!(
                        "activities from {title} include a record with no activity id"
                    ));
                }
            }
            Err(error) => {
                log::warn!("morning briefing: activities for {facet} {day} unreadable: {error}");
                gaps.push(format!("activities from {title}, which couldn't be read"));
            }
        }
    }
    activities.sort_by(|(idx_a, act_a), (idx_b, act_b)| {
        idx_a
            .cmp(idx_b)
            .then_with(|| match (&act_a.start, &act_b.start) {
                (Some(sa), Some(sb)) => sa.cmp(sb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| act_a.id.cmp(&act_b.id))
    });
    activities.into_iter().map(|(_, act)| act).collect()
}

/// The names of an activity's attendees.
fn attendee_names(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("participation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|entry| entry.get("role").and_then(Value::as_str) == Some("attendee"))
        .map(|entry| string_or(entry.get("name").or_else(|| entry.get("entity_id")), ""))
        .filter(|v| !v.is_empty())
        .collect()
}

/// One scheduled event as a digest line: its title, its time and the known
/// people invited.
fn describe_scheduled(event: &Value) -> String {
    let mut text = string_or(event.get("title"), "untitled event");
    if let (Some(start), Some(end)) = (
        event.get("start").and_then(Value::as_str),
        event.get("end").and_then(Value::as_str),
    ) {
        text.push_str(&format!(", {start}–{end}"));
    }
    let people = event
        .get("people")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if !people.is_empty() {
        text.push_str(&format!(", invited: {}", people.join(", ")));
    }
    text
}

fn render_digest_activity(act: &DigestActivity, include_story: bool) -> String {
    let mut lines = Vec::new();
    let title_line = if let Some(ref title) = act.title {
        format!("### {title}")
    } else {
        "### Untitled activity".to_string()
    };
    lines.push(title_line);
    lines.push(format!("- id: {}", act.id));
    lines.push(format!(
        "- source: sol://facets/{}/activities/{}#{}",
        act.facet, act.day, act.id
    ));
    if let Some(ref start) = act.start {
        lines.push(format!("- start: {start}"));
    }
    if let Some(ref end) = act.end {
        lines.push(format!("- end: {end}"));
    }
    if let Some(ref target_date) = act.target_date {
        lines.push(format!("- target_date: {target_date}"));
    }
    if let Some(ref dur) = act.duration {
        lines.push(format!("- duration: {dur}"));
    }
    if let Some(ref desc) = act.description {
        lines.push(format!("- description: {desc}"));
    }
    if let Some(ref det) = act.details {
        lines.push(format!("- details: {det}"));
    }
    if !act.participants.is_empty() {
        lines.push(format!("- participants: {}", act.participants.join(", ")));
    }
    for scheduled in &act.scheduled {
        lines.push(format!(
            "- on the owner's calendar then (scheduled, not who attended): {scheduled}"
        ));
    }
    if !act.entities.is_empty() {
        lines.push(format!("- entities: {}", act.entities.join(", ")));
    }
    if act.source_limitation {
        lines.push("source_limitation: partial_input".to_string());
    }
    if include_story && let Some(ref body) = act.story_body {
        lines.push(format!("Story:\n{body}"));
    }
    lines.join("\n")
}

pub(crate) fn render_digest_activities(
    activities: &[DigestActivity],
    include_activities: &[bool],
    include_stories: &[bool],
) -> String {
    let mut items = Vec::new();
    for (i, act) in activities.iter().enumerate() {
        if include_activities.get(i).copied().unwrap_or(true) {
            let include_story = include_stories.get(i).copied().unwrap_or(true);
            items.push(render_digest_activity(act, include_story));
        }
    }
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join("\n\n")
    }
}

/// When the briefing was prepared, as a wall time on the journal's clock; home
/// shows its hour and minute as written.
fn generated_stamp(home: &HomeContext) -> String {
    home.now_local().format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn read_pulse(home: &HomeContext, day: &str, gaps: &mut Vec<String>) -> String {
    let Some(record) = read_latest(home, day, "pulse", 0) else {
        gaps.push("the latest pulse".into());
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(details) = record
        .get("full_details")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(details.to_owned());
    }
    let needs = record
        .get("needs_you")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| value.to_string().trim_matches('"').trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if !needs.is_empty() {
        parts.push(format!(
            "Needs you:\n{}",
            needs
                .iter()
                .map(|value| format!("- {value}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let value = parts.join("\n\n");
    if value.is_empty() {
        gaps.push("the latest pulse".into());
    }
    value
}

fn render_facets(facets: &[(String, String)]) -> String {
    if facets.is_empty() {
        "(none)".into()
    } else {
        facets
            .iter()
            .map(|(name, title)| format!("- {name}: {title}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The owner's note on what this briefing could not draw on.
const ACTIVITIES_OMITTED: &str = "activities omitted: ";
const STORIES_OMITTED: &str = "activity stories omitted: ";

/// Counts, not record ids: one gap per omitted record grows with the day, and a
/// day with hundreds of activities could never fit the floor even with all of
/// them left out. Which records were left out is in `_briefing_inclusion`.
pub(crate) fn omission_gaps(
    activities: &[DigestActivity],
    include_activities: &[bool],
    include_stories: &[bool],
) -> Vec<String> {
    let omitted = include_activities.iter().filter(|kept| !**kept).count();
    let stories = activities
        .iter()
        .enumerate()
        .filter(|(i, act)| {
            include_activities[*i] && !include_stories[*i] && act.story_body.is_some()
        })
        .count();
    let mut gaps = Vec::new();
    if omitted > 0 {
        gaps.push(format!("{ACTIVITIES_OMITTED}{omitted}."));
    }
    if stories > 0 {
        gaps.push(format!("{STORIES_OMITTED}{stories}."));
    }
    gaps
}

/// Whether `gap` is an omission gap, in the count form or the per-record form
/// earlier packets stored.
pub(crate) fn is_omission_gap(gap: &str, activities: &[DigestActivity]) -> bool {
    gap.starts_with(ACTIVITIES_OMITTED)
        || gap.starts_with(STORIES_OMITTED)
        || activities.iter().any(|act| {
            gap.strip_prefix("activity ")
                .or_else(|| gap.strip_prefix("story for "))
                == Some(act.id.as_str())
        })
}

pub(crate) fn coverage_preamble(gaps: &[String]) -> String {
    let mut parts = Vec::new();
    let (mut activities, mut stories) = (0, 0);
    let count = |gap: &str, prefix: &str| {
        gap.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix('.'))
            .and_then(|count| count.parse::<usize>().ok())
    };
    for gap in gaps {
        if let Some(omitted) = count(gap, ACTIVITIES_OMITTED) {
            activities += omitted;
        } else if let Some(omitted) = count(gap, STORIES_OMITTED) {
            stories += omitted;
        } else if gap.starts_with("story for ") {
            stories += 1;
        } else if gap.starts_with("activity ") {
            activities += 1;
        } else if let Some(count) = gap
            .strip_prefix("location entries omitted: ")
            .and_then(|rest| rest.strip_suffix('.'))
        {
            parts.push(if count == "1" {
                "one location entry".to_owned()
            } else {
                format!("{count} location entries")
            });
        } else if gap == "the latest pulse" {
            parts.push("the latest running summary".to_owned());
        } else {
            parts.push(gap.trim_end_matches('.').to_owned());
        }
    }
    if activities > 0 {
        parts.push(if activities == 1 {
            "one activity".to_owned()
        } else {
            format!("{activities} activities")
        });
    }
    if stories > 0 {
        parts.push(if stories == 1 {
            "the story of one activity".to_owned()
        } else {
            format!("the stories of {stories} activities")
        });
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("Not in this briefing: {}.", parts.join("; "))
}

fn string_or(value: Option<&Value>, fallback: &str) -> String {
    value.and_then(Value::as_str).unwrap_or(fallback).to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Two builds can straddle a second, and `generated` is the wall clock: the
    /// packets must agree on everything else.
    fn without_generated(mut values: Map<String, Value>) -> Map<String, Value> {
        if let Some(Value::String(text)) = values.get("briefing_metadata") {
            let mut metadata: Value = serde_json::from_str(text).unwrap();
            metadata.as_object_mut().unwrap().remove("generated");
            values.insert("briefing_metadata".into(), metadata);
        }
        values
    }

    #[test]
    fn coverage_preamble_names_only_what_is_missing() {
        assert_eq!(coverage_preamble(&[]), "");
        assert_eq!(
            coverage_preamble(&["the latest pulse".to_owned()]),
            "Not in this briefing: the latest running summary."
        );
    }

    #[test]
    fn coverage_preamble_counts_omitted_records_without_their_ids() {
        let gaps = [
            "the latest pulse",
            "story for 20261019_meeting_1",
            "activity 20261019_x",
            "story for 20261019_y",
            "location entries omitted: 3.",
        ]
        .map(str::to_owned);
        assert_eq!(
            coverage_preamble(&gaps),
            "Not in this briefing: the latest running summary; 3 location entries; \
             one activity; the stories of 2 activities."
        );
    }

    #[test]
    fn generated_reads_the_journals_clock_not_this_computers() {
        let instant = chrono::DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let host_day = instant
            .with_timezone(&solstone_core_journal_config::host_zone())
            .date_naive();
        let zone = [
            solstone_core_journal_config::Tz::Pacific__Kiritimati,
            solstone_core_journal_config::Tz::Etc__GMTPlus12,
        ]
        .into_iter()
        .find(|zone| instant.with_timezone(zone).date_naive() != host_day)
        .expect("UTC+14 and UTC-12 never share a date");
        let home = HomeContext::with_zone("journal", instant, zone);
        assert_eq!(
            generated_stamp(&home),
            instant
                .with_timezone(&zone)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        );
    }

    #[test]
    fn packet_reads_the_analysis_day_and_never_a_retained_plan() {
        let root = tempfile::TempDir::new().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let observed = json!({"id":"seen","title":"observed work"}).to_string();
        for (day, title) in [
            ("20261231", "analysis plan"),
            ("20270101", "presentation plan"),
            ("20270102", "forward plan"),
        ] {
            let plan = json!({"id":day,"source":"anticipated","title":title}).to_string();
            let rows = if day == "20261231" {
                format!("{observed}\n{plan}\n")
            } else {
                format!("{plan}\n")
            };
            fs::write(facet.join(format!("activities/{day}.jsonl")), rows).unwrap();
        }
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let values = build_packet(
            "20261231",
            NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            "test",
            &context,
        )
        .unwrap();
        assert_eq!(values["briefing_analysis_day"], "20261231");
        assert_eq!(values["briefing_presentation_day"], "20270101");
        let digest = values["activity_digest"].as_str().unwrap();
        assert!(digest.contains("observed work"), "{digest}");
        let text = serde_json::to_string(&values).unwrap();
        assert!(!text.contains(" plan"), "{text}");
        assert!(values.get("anticipated_today").is_none());
        assert!(values.get("anticipated_forward").is_none());
    }

    #[test]
    fn gate_keeps_reference_day_reasons() {
        let prepared = PreparedTalent::new("morning_briefing".into(), Map::new());
        let context = ExecutionContext {
            journal: Default::default(),
        };
        assert_eq!(
            gate(&prepared, &context).unwrap(),
            GateDecision::Skip("missing day".into())
        );
    }

    // 1. Populated analysis day: title, description, times, participants, entities, story body,
    // and sol://facets/.../activities/{day}#{id} appear. A second build is identical.
    #[test]
    fn test_populated_day_digest() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let record = json!({
            "id": "act_1",
            "title": "Quarterly Review",
            "description": "Review Q3 results",
            "details": "Detailed financial breakdown",
            "start": "09:00",
            "end": "10:00",
            "target_date": "20261019",
            "duration": 60,
            "participation": [
                {"name": "Alice", "role": "attendee"},
                {"name": "Perry Evans", "role": "mentioned"},
            ],
            "active_entities": ["Acme Corp"],
            "story": {"body": "Alice presented the financial metrics."},
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record}\n"),
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values1 = build_packet("20261019", date, "test", &context).unwrap();
        let values2 = build_packet("20261019", date, "test", &context).unwrap();
        assert_eq!(
            without_generated(values1.clone()),
            without_generated(values2.clone())
        );

        let digest = values1["activity_digest"].as_str().unwrap();
        assert!(digest.contains("Quarterly Review"));
        assert!(digest.contains("Review Q3 results"));
        assert!(digest.contains("Detailed financial breakdown"));
        assert!(digest.contains("09:00"));
        assert!(digest.contains("10:00"));
        assert!(digest.contains("20261019"));
        assert!(digest.contains("60"));
        assert!(digest.contains("- participants: Alice\n"));
        assert!(!digest.contains("Perry Evans"));
        assert!(digest.contains("Acme Corp"));
        assert!(digest.contains("Alice presented the financial metrics."));
        assert!(digest.contains("sol://facets/work/activities/20261019#act_1"));
    }

    // 2. Empty facets: stable across two builds, no omission gap, failed-facet wording absent.
    #[test]
    fn test_empty_facets_stable_no_gap() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(&facet).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values1 = build_packet("20261019", date, "test", &context).unwrap();
        let values2 = build_packet("20261019", date, "test", &context).unwrap();
        assert_eq!(
            without_generated(values1.clone()),
            without_generated(values2.clone())
        );

        let metadata: Value =
            serde_json::from_str(values1["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps = metadata["gaps"].as_array().unwrap();
        for gap in gaps {
            let g = gap.as_str().unwrap();
            assert!(!g.contains("activities"));
            assert!(!g.contains("couldn't be read"));
        }
        assert_eq!(values1["activity_digest"], "(none)");
    }

    // 3. Two facets: order follows declaration order, then start, then id, including a missing start sorting last. Stable across two builds.
    #[test]
    fn test_two_facets_sorting_order() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("facets/work");
        fs::create_dir_all(work.join("activities")).unwrap();
        fs::write(work.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let personal = root.path().join("facets/personal");
        fs::create_dir_all(personal.join("activities")).unwrap();
        fs::write(personal.join("facet.json"), r#"{"title":"Personal"}"#).unwrap();

        let work_records = [
            json!({"id": "act_c", "start": "14:00"}),
            json!({"id": "act_d"}),
            json!({"id": "act_b", "start": "09:00"}),
            json!({"id": "act_a"}),
        ];
        let mut work_jsonl = String::new();
        for r in &work_records {
            work_jsonl.push_str(&format!("{r}\n"));
        }
        fs::write(work.join("activities/20261019.jsonl"), work_jsonl).unwrap();

        let personal_records = [
            json!({"id": "act_p2", "start": "08:00"}),
            json!({"id": "act_p1", "start": "08:00"}),
        ];
        let mut personal_jsonl = String::new();
        for r in &personal_records {
            personal_jsonl.push_str(&format!("{r}\n"));
        }
        fs::write(personal.join("activities/20261019.jsonl"), personal_jsonl).unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values1 = build_packet("20261019", date, "test", &context).unwrap();
        let values2 = build_packet("20261019", date, "test", &context).unwrap();
        assert_eq!(
            without_generated(values1.clone()),
            without_generated(values2.clone())
        );

        let digest = values1["activity_digest"].as_str().unwrap();
        let pos_b = digest.find("act_b").unwrap();
        let pos_c = digest.find("act_c").unwrap();
        let pos_a = digest.find("act_a").unwrap();
        let pos_d = digest.find("act_d").unwrap();
        let pos_p1 = digest.find("act_p1").unwrap();
        let pos_p2 = digest.find("act_p2").unwrap();

        assert!(
            pos_p1 < pos_p2,
            "p1 ({pos_p1}) should precede p2 ({pos_p2})"
        );
        assert!(pos_p2 < pos_b, "p2 ({pos_p2}) should precede b ({pos_b})");
        assert!(pos_b < pos_c, "b ({pos_b}) should precede c ({pos_c})");
        assert!(pos_c < pos_a, "c ({pos_c}) should precede a ({pos_a})");
        assert!(pos_a < pos_d, "a ({pos_a}) should precede d ({pos_d})");
    }

    // 4. Oversize stories: measured prompt + schema fit compute_input_budget(max_output_tokens, LOCAL_MIN_CONTEXT_TOKENS).
    // A gap names an omitted id.
    // Story bodies are absent once dropped; no mid-string remnant of a dropped story.
    #[test]
    fn a_day_with_hundreds_of_activities_fits_the_floor() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let records = (0..1200)
            .map(|i| {
                json!({
                    "id": format!("20261019_activity_{i:04}"),
                    "title": format!("Activity {i}"),
                    "start": "09:00",
                    "story": {"body": "a short story about what happened in this activity"},
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(facet.join("activities/20261019.jsonl"), records).unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let mut prepared = PreparedTalent::new(
            "morning_briefing".into(),
            Map::from_iter([
                ("day".into(), json!("20261019")),
                ("max_output_tokens".into(), json!(2560)),
                ("json_schema".into(), json!({"type": "object"})),
                (
                    "prompt".into(),
                    json!("Prompt with $activity_digest and $briefing_metadata"),
                ),
            ]),
        );
        let state = build(&mut prepared, &context).unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();

        let metadata: Value =
            serde_json::from_str(prepared.config["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps = metadata["gaps"].as_array().unwrap();
        assert!(
            gaps.iter().any(|gap| gap
                .as_str()
                .is_some_and(|gap| gap.starts_with(ACTIVITIES_OMITTED))),
            "{gaps:?}"
        );
        assert!(
            gaps.len() < 10,
            "omissions are counted, not listed: {gaps:?}"
        );
    }

    #[test]
    fn test_oversize_story_clipping() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let huge_story = "A".repeat(50000);
        let record1 = json!({
            "id": "act_huge",
            "title": "First Huge Activity",
            "start": "09:00",
            "story": {"body": huge_story},
        });
        let record2 = json!({
            "id": "act_small",
            "title": "Second Activity",
            "start": "10:00",
            "story": {"body": "short story content"},
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record1}\n{record2}\n"),
        )
        .unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let mut prepared = PreparedTalent::new(
            "morning_briefing".into(),
            Map::from_iter([
                ("day".into(), json!("20261019")),
                ("max_output_tokens".into(), json!(1024)),
                ("json_schema".into(), json!({"type": "object"})),
                (
                    "prompt".into(),
                    json!("Prompt with $activity_digest and $briefing_metadata"),
                ),
                ("_daily_location_text".into(), json!("Denver, Colorado")),
            ]),
        );
        let state = build(&mut prepared, &context).unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();

        let prompt = prepared.config["prompt"].as_str().unwrap();
        let schema_str = serde_json::to_string(&prepared.config["json_schema"]).unwrap();
        let budget = solstone_core_local::generate::compute_input_budget(
            1024,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        );
        let total_tokens: u32 = crate::model_text_parts(&prepared.config)
            .iter()
            .map(|part| solstone_core_local::generate::estimate_tokens(part))
            .sum::<u32>()
            + solstone_core_local::generate::estimate_tokens(&schema_str);
        assert!(
            total_tokens <= budget,
            "tokens {total_tokens} must fit budget {budget}"
        );
        assert!(prompt.contains(STORIES_OMITTED) || prompt.contains(ACTIVITIES_OMITTED));
        assert!(!prompt.contains(&huge_story));
        assert!(!prompt.contains(&huge_story[..100]));
    }

    #[test]
    fn test_non_digest_remainder_cannot_fit_returns_budget_error() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let huge_instruction = "X".repeat(100_000);
        let mut prepared = PreparedTalent::new(
            "morning_briefing".into(),
            Map::from_iter([
                ("day".into(), json!("20261019")),
                ("max_output_tokens".into(), json!(1024)),
                ("json_schema".into(), json!({"type": "object"})),
                ("user_instruction".into(), json!(huge_instruction)),
                ("prompt".into(), json!("Short prompt")),
            ]),
        );
        let state = build(&mut prepared, &context).unwrap();
        let result = apply_prompt_override(&mut prepared, &state);
        assert!(
            result.is_err(),
            "expected budget error when request cannot fit floor budget"
        );
        let err = result.unwrap_err();
        assert!(err.detail.contains("floor input budget"));
        assert_eq!(prepared.config["prompt"], json!("Short prompt"));
    }

    #[test]
    fn an_activity_carries_the_meeting_it_ran_during_as_scheduled_not_attended() {
        let act = DigestActivity {
            facet: "work".into(),
            day: "20261019".into(),
            id: "act".into(),
            start: None,
            end: None,
            target_date: None,
            duration: None,
            title: Some("Talked through the mocks".into()),
            description: None,
            details: None,
            participants: vec![],
            entities: vec![],
            source_limitation: false,
            story_body: None,
            scheduled: vec![describe_scheduled(&json!({
                "title": "design review", "start": "10:00", "end": "11:00",
                "people": ["Alex Rivera", "Dana Ortiz"],
            }))],
        };
        let rendered = render_digest_activity(&act, false);
        let line = rendered
            .lines()
            .find(|line| line.contains("design review"))
            .unwrap_or_else(|| panic!("{rendered}"));
        for part in ["scheduled", "10:00", "11:00", "Alex Rivera", "Dana Ortiz"] {
            assert!(line.contains(part), "{line}");
        }
        // Invitees are never offered as people who took part.
        assert!(!rendered.contains("participants"));
    }

    #[test]
    fn a_person_only_mentioned_is_never_offered_as_a_participant() {
        let record = json!({
            "participation": [
                {"name": "Ada Lovelace", "role": "attendee", "source": "voice"},
                {"name": "Perry Evans", "role": "mentioned", "source": "screen"},
                {"name": "Grace Hopper", "source": "screen"},
                {"entity_id": "atlas", "role": "attendee"},
            ],
        });
        assert_eq!(
            attendee_names(record.as_object().unwrap()),
            vec!["Ada Lovelace", "atlas"]
        );
    }

    #[test]
    fn test_source_gaps_not_substring_deleted() {
        let huge_story = "A".repeat(50000);
        let act = DigestActivity {
            facet: "work".into(),
            day: "20261019".into(),
            id: "act".into(),
            start: Some("09:00".into()),
            end: None,
            target_date: None,
            duration: None,
            title: Some("Act Title".into()),
            description: None,
            details: None,
            participants: vec![],
            entities: vec![],
            source_limitation: false,
            story_body: Some(huge_story),
            scheduled: vec![],
        };
        let initial_gap = "activities from Work, which couldn't be read".to_string();
        let values = Map::from_iter([
            ("briefing_analysis_day".into(), json!("20261019")),
            ("briefing_presentation_day".into(), json!("20261020")),
            ("briefing_metadata".into(), json!("{}")),
            ("activity_digest".into(), json!("")),
            ("pulse_surface".into(), json!("")),
        ]);
        let state = PrePostState::MorningBriefing(MorningBriefingPreState {
            values,
            activities: vec![act],
            initial_gaps: vec![initial_gap.clone()],
            counts: json!({"segments": 0}),
            generated: "2026-10-20T07:00:00Z".into(),
            model: "test-model".into(),
        });
        let mut prepared = PreparedTalent::new(
            "morning_briefing".into(),
            Map::from_iter([
                ("day".into(), json!("20261019")),
                ("max_output_tokens".into(), json!(1024)),
                ("json_schema".into(), json!({"type": "object"})),
                (
                    "prompt".into(),
                    json!("Prompt with $activity_digest and $briefing_metadata"),
                ),
            ]),
        );
        apply_prompt_override(&mut prepared, &state).unwrap();
        let prompt = prepared.config["prompt"].as_str().unwrap();
        assert!(
            prompt.contains(&initial_gap),
            "exact source gap must still be present"
        );
        assert!(prompt.contains("act"), "omission must name act");
    }

    // 5. One valid facet and one malformed activity file: valid activity id is in the digest;
    // a gap names the failed facet; the gap does not say the day was empty or that there were no activities.
    #[test]
    fn test_one_valid_one_malformed_activity_file() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("facets/work");
        fs::create_dir_all(work.join("activities")).unwrap();
        fs::write(work.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let record = json!({"id": "valid_act", "title": "Valid Plan"});
        fs::write(
            work.join("activities/20261019.jsonl"),
            format!("{record}\n"),
        )
        .unwrap();

        let personal = root.path().join("facets/personal");
        fs::create_dir_all(personal.join("activities")).unwrap();
        fs::write(personal.join("facet.json"), r#"{"title":"Personal"}"#).unwrap();
        fs::write(personal.join("activities/20261019.jsonl"), [0xff, 0xff]).unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();
        let digest = values["activity_digest"].as_str().unwrap();
        assert!(digest.contains("valid_act"));

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps = metadata["gaps"].as_array().unwrap();
        let gap_strs: Vec<&str> = gaps.iter().filter_map(Value::as_str).collect();
        assert!(
            gap_strs
                .iter()
                .any(|g| g.contains("Personal") && g.contains("couldn't be read"))
        );
        for g in &gap_strs {
            if g.contains("Personal") {
                assert!(!g.contains("empty"));
                assert!(!g.contains("no activities"));
            }
        }
    }

    #[test]
    fn test_mixed_jsonl_keeps_valid_records_and_reports_incomplete_gap() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let record1 = json!({"id": "act_valid_1", "title": "First Valid"});
        let record2 = json!({"id": "act_valid_2", "title": "Second Valid"});
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record1}\nnot-json\n{record2}\n"),
        )
        .unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();
        let digest = values["activity_digest"].as_str().unwrap();
        assert!(
            digest.contains("act_valid_1"),
            "first valid activity must be in digest"
        );
        assert!(
            digest.contains("act_valid_2"),
            "second valid activity must be in digest"
        );

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps: Vec<&str> = metadata["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            gaps.iter()
                .any(|g| g.contains("Work") && g.contains("include a line that could not be read")),
            "incomplete gap must name the facet: {gaps:?}"
        );
    }

    #[test]
    fn test_valid_jsonl_twin_has_no_incomplete_gap() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let record1 = json!({"id": "act_valid_1", "title": "First Valid"});
        let record2 = json!({"id": "act_valid_2", "title": "Second Valid"});
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record1}\n{record2}\n"),
        )
        .unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();
        let digest = values["activity_digest"].as_str().unwrap();
        assert!(digest.contains("act_valid_1"));
        assert!(digest.contains("act_valid_2"));

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps: Vec<&str> = metadata["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            !gaps
                .iter()
                .any(|g| g.contains("could not be read") || g.contains("couldn't be read")),
            "twin file with only valid rows must have no incomplete gap: {gaps:?}"
        );
        assert!(
            !gaps.iter().any(|g| g.contains("no activity id")),
            "twin file with only valid rows must have no missing activity id gap: {gaps:?}"
        );
    }

    #[test]
    fn test_mixed_jsonl_missing_activity_id_keeps_valid_records() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let row1 = json!({"id": "act_valid", "title": "Kept"});
        let row2 = json!({"title": "unreferenceable activity"});
        let row3 = json!({"id": 1, "title": "numeric"});
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{row1}\n{row2}\n{row3}\n"),
        )
        .unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();
        let digest = values["activity_digest"].as_str().unwrap();
        assert!(
            digest.contains("act_valid"),
            "valid activity must be in digest"
        );
        assert!(
            !digest.contains("unreferenceable activity"),
            "unreferenceable activity must not be in digest"
        );
        assert!(
            !digest.contains("numeric"),
            "numeric id activity must not be in digest"
        );

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps: Vec<&str> = metadata["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let expected_gap = "activities from Work include a record with no activity id";
        assert!(
            gaps.contains(&expected_gap),
            "expected gap '{expected_gap}' in {gaps:?}"
        );
        for g in &gaps {
            if g.contains("Work") {
                assert!(
                    !g.contains("empty"),
                    "gap must not describe an empty day: {g}"
                );
                assert!(
                    !g.contains("no activities"),
                    "gap must not say no activities: {g}"
                );
            }
        }
    }

    #[test]
    fn test_jsonl_only_missing_activity_id_reports_gap() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let row = json!({"title": "unreferenceable activity"});
        fs::write(facet.join("activities/20261019.jsonl"), format!("{row}\n")).unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();
        let digest = values["activity_digest"].as_str().unwrap();
        assert_eq!(digest, "(none)");

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps: Vec<&str> = metadata["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let expected_gap = "activities from Work include a record with no activity id";
        assert!(
            gaps.contains(&expected_gap),
            "expected gap '{expected_gap}' in {gaps:?}"
        );
        for g in &gaps {
            if g.contains("Work") {
                assert!(
                    !g.contains("empty"),
                    "gap must not describe an empty day: {g}"
                );
                assert!(
                    !g.contains("no activities"),
                    "gap must not say no activities: {g}"
                );
            }
        }
    }

    #[test]
    fn test_jsonl_only_not_json_reports_gap_not_empty_day() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        fs::write(facet.join("activities/20261019.jsonl"), "not-json\n").unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values = build_packet("20261019", date, "test", &context).unwrap();

        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        let gaps: Vec<&str> = metadata["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            gaps.iter()
                .any(|g| g.contains("Work") && g.contains("include a line that could not be read")),
            "gap must be present: {gaps:?}"
        );
        for g in &gaps {
            if g.contains("Work") {
                assert!(
                    !g.contains("empty"),
                    "gap must not describe an empty day: {g}"
                );
                assert!(
                    !g.contains("no activities"),
                    "gap must not say no activities: {g}"
                );
            }
        }
    }

    // 6. partial_input object plus non-empty body: digest contains source_limitation: partial_input.
    // The complete-input twin does not contain partial_input.
    #[test]
    fn test_partial_input_limitation_predicate() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let record1 = json!({
            "id": "act_partial",
            "title": "Meeting",
            "story": {
                "body": "Meeting content was long.",
                "partial_input": {"reason": "cut"}
            }
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record1}\n"),
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values1 = build_packet("20261019", date, "test", &context).unwrap();
        let digest1 = values1["activity_digest"].as_str().unwrap();
        assert!(digest1.contains("source_limitation: partial_input"));

        // Complete-input twin
        let record2 = json!({
            "id": "act_complete",
            "title": "Meeting",
            "story": {
                "body": "Meeting content was long."
            }
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record2}\n"),
        )
        .unwrap();
        let values2 = build_packet("20261019", date, "test", &context).unwrap();
        let digest2 = values2["activity_digest"].as_str().unwrap();
        assert!(!digest2.contains("partial_input"));
    }

    // 7. Same activities, change only the news file: packet equal, and the news body is not in the packet.
    // Change only a title or story body: packet changes.
    #[test]
    fn test_news_file_independence() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::create_dir_all(facet.join("news")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();

        let record = json!({
            "id": "act_1",
            "title": "Initial Title",
            "story": {"body": "Initial Story Body"}
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record}\n"),
        )
        .unwrap();
        fs::write(facet.join("news/20261019.md"), "Initial News File Content").unwrap();

        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let values1 = build_packet("20261019", date, "test", &context).unwrap();

        // Change only the news file
        fs::write(
            facet.join("news/20261019.md"),
            "Completely Different News Content",
        )
        .unwrap();
        let values2 = build_packet("20261019", date, "test", &context).unwrap();
        assert_eq!(
            without_generated(values1.clone()),
            without_generated(values2.clone())
        );
        let digest = values1["activity_digest"].as_str().unwrap();
        assert!(!digest.contains("News"));

        // Change title
        let record_updated = json!({
            "id": "act_1",
            "title": "Changed Title",
            "story": {"body": "Initial Story Body"}
        });
        fs::write(
            facet.join("activities/20261019.jsonl"),
            format!("{record_updated}\n"),
        )
        .unwrap();
        let values3 = build_packet("20261019", date, "test", &context).unwrap();
        assert_ne!(values1, values3);
    }
}
