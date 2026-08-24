// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Morning-briefing pre-hook.

use std::collections::BTreeSet;
use std::fs;

use chrono::{Duration, NaiveDate, Utc};
use serde_json::{Map, Value, json};
use solstone_core_facets::{load_activity_records, read_facet_declaration, read_news_file};
use solstone_core_home::{
    HomeContext,
    readers::{enabled_facet_names, read_latest},
};
use solstone_core_indexer_query::{SearchHit, SearchRequest, search};

use crate::contract::{GateDecision, PrePostState};
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MorningBriefingPreState {
    values: Map<String, Value>,
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
    build_packet(
        &day,
        analysis_day,
        prepared
            .config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        context,
    )
    .map(|values| PrePostState::MorningBriefing(MorningBriefingPreState { values }))
    .map_err(|error| RuntimeOutcome::Skipped {
        stage: "morning_briefing".into(),
        talent: prepared.name.clone(),
        reason: format!("morning briefing pre-hook failed: {error}"),
    })
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::MorningBriefing(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "morning_briefing",
            prepared,
            "missing morning briefing state",
        ));
    };
    apply_template_vars(&mut prepared.config, &state.values);
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

fn build_packet(
    day: &str,
    analysis_day: NaiveDate,
    model: &str,
    context: &ExecutionContext,
) -> Result<Map<String, Value>, String> {
    let mut gaps = Vec::new();
    let home = HomeContext::new(&context.journal, Utc::now());
    // `enabled_facet_names` supplies the reference's declared-name + muted filter.
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
        gaps.push("no active facets available".to_owned());
    }
    let newsletters = load_newsletters(&facets, day, context, &mut gaps);
    let today = load_activities(
        &facets,
        &[day.to_owned()],
        context,
        &mut gaps,
        "no anticipated activities today",
    );
    let forward_days = (1..8)
        .map(|offset| {
            (analysis_day + Duration::days(offset))
                .format("%Y%m%d")
                .to_string()
        })
        .collect::<Vec<_>>();
    let forward = load_activities(
        &facets,
        &forward_days,
        context,
        &mut gaps,
        "no anticipated activities in the next 7 days",
    );
    let (followups_total, followups) =
        search_agent(day, "followups", "follow-up items", context, &mut gaps);
    let (decisions_total, decisions) =
        search_agent(day, "decisions", "decision items", context, &mut gaps);
    let pulse = read_pulse(&home, day, &mut gaps);
    let partner = read_identity(&context.journal, "partner.md", "partner profile", &mut gaps);
    let health = read_identity(
        &context.journal,
        "health.md",
        "steward health surface",
        &mut gaps,
    );
    let paths = followups
        .iter()
        .chain(&decisions)
        .filter_map(|result| {
            (!result.metadata.path.is_empty())
                .then_some(result.metadata.path.clone())
                .or_else(|| (!result.id.is_empty()).then_some(result.id.clone()))
        })
        .collect::<BTreeSet<_>>();
    let counts = json!({"segments": paths.len(), "anticipated_activities": today.len(), "facet_newsletters": newsletters.len(), "followups": followups.len(), "steward_health": if health.is_empty() { "missing" } else { "present" }});
    let metadata = json!({"generated": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(), "model": model, "sources": counts, "gaps": gaps, "coverage_preamble": coverage_preamble(&counts, &gaps, decisions_total, forward.len(), followups_total)});
    Ok(Map::from_iter([
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
            "facet_newsletters".into(),
            Value::String(render_newsletters(&newsletters, day)),
        ),
        (
            "anticipated_today".into(),
            Value::String(render_activities(&today, false)),
        ),
        (
            "anticipated_forward".into(),
            Value::String(render_activities(&forward, true)),
        ),
        (
            "pulse_surface".into(),
            Value::String(if pulse.is_empty() {
                "(missing)".into()
            } else {
                pulse
            }),
        ),
        (
            "partner_surface".into(),
            Value::String(if partner.is_empty() {
                "(missing)".into()
            } else {
                partner
            }),
        ),
        (
            "health_surface".into(),
            Value::String(if health.is_empty() {
                "(missing)".into()
            } else {
                health
            }),
        ),
        ("followups".into(), Value::String(render_search(&followups))),
        ("decisions".into(), Value::String(render_search(&decisions))),
    ]))
}

fn load_newsletters(
    facets: &[(String, String)],
    day: &str,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> Vec<(String, String)> {
    let mut newsletters = Vec::new();
    for (facet, _) in facets {
        match read_news_file(&context.journal, facet, &format!("{day}.md")) {
            Ok(Some(content)) if !content.trim().is_empty() => {
                newsletters.push((facet.clone(), content.trim().to_owned()))
            }
            Ok(_) => gaps.push(format!("no facet newsletter available for {facet}")),
            Err(error) => gaps.push(format!("facet newsletter unavailable for {facet}: {error}")),
        }
    }
    if !facets.is_empty() && newsletters.is_empty() {
        gaps.push("no facet newsletters available".into());
    }
    newsletters
}
fn load_activities(
    facets: &[(String, String)],
    days: &[String],
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
    empty_gap: &str,
) -> Vec<Value> {
    let mut values = Vec::new();
    for day in days {
        for (facet, _) in facets {
            match load_activity_records(&context.journal, facet, day, false) {
                Ok(records) => {
                    for mut record in records.into_iter().filter(|record| {
                        record.get("source").and_then(Value::as_str) == Some("anticipated")
                    }) {
                        record.insert(
                            "facet".into(),
                            Value::String(string_or(record.get("facet"), facet)),
                        );
                        record.insert(
                            "day".into(),
                            Value::String(string_or(record.get("target_date"), day)),
                        );
                        values.push(Value::Object(record));
                    }
                }
                Err(error) => gaps.push(format!(
                    "anticipated activities unavailable for {facet} {day}: {error}"
                )),
            }
        }
    }
    values.sort_by_key(|value| {
        (
            string_or(value.get("day"), ""),
            string_or(value.get("start"), ""),
            string_or(value.get("facet"), ""),
            string_or(value.get("title"), ""),
        )
    });
    if !facets.is_empty() && values.is_empty() {
        gaps.push(empty_gap.into());
    }
    values
}
fn search_agent(
    day: &str,
    agent: &str,
    label: &str,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> (u64, Vec<SearchHit>) {
    let mut request = SearchRequest::new("", Default::default());
    request.limit = 10;
    request.day = Some(day.into());
    request.agent = Some(agent.into());
    request.counts = true;
    match search(&context.journal, &request, Utc::now().date_naive()) {
        Ok(response) => {
            if response.results.is_empty() {
                gaps.push(format!("no {label} found"));
            }
            (response.total.unwrap_or(0), response.results)
        }
        Err(error) => {
            gaps.push(format!("{label} search unavailable: {error}"));
            (0, Vec::new())
        }
    }
}
fn read_identity(
    journal: &std::path::Path,
    name: &str,
    label: &str,
    gaps: &mut Vec<String>,
) -> String {
    match fs::read_to_string(journal.join("identity").join(name)) {
        Ok(content) if !content.trim().is_empty() => content.trim().into(),
        Ok(_) => {
            gaps.push(format!("{label} empty"));
            String::new()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            gaps.push(format!("{label} missing"));
            String::new()
        }
        Err(error) => {
            gaps.push(format!("{label} unavailable: {error}"));
            String::new()
        }
    }
}
fn read_pulse(home: &HomeContext, day: &str, gaps: &mut Vec<String>) -> String {
    let Some(record) = read_latest(home, day, "pulse", 0) else {
        gaps.push("pulse surface".into());
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
        gaps.push("pulse surface".into());
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
fn render_newsletters(values: &[(String, String)], day: &str) -> String {
    if values.is_empty() {
        "(none)".into()
    } else {
        values
            .iter()
            .map(|(facet, content)| {
                format!(
                    "### {facet} newsletter\nSource: sol://facets/{facet}/news/{day}\n{content}",
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
fn render_activities(values: &[Value], grouped: bool) -> String {
    if values.is_empty() {
        return "(none)".into();
    }
    let mut lines = Vec::new();
    let mut prior = String::new();
    for value in values {
        let day = string_or(value.get("day"), "");
        if grouped && day != prior {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("### {day}"));
            prior = day;
        }
        let start = short_time(value.get("start"));
        let end = short_time(value.get("end"));
        let time = if !start.is_empty() && !end.is_empty() {
            format!("{start}-{end}")
        } else if !start.is_empty() {
            start
        } else {
            "unscheduled".into()
        };
        let title = string_or(
            value.get("title").or_else(|| value.get("activity")),
            "Untitled activity",
        );
        let activity = string_or(value.get("activity"), "activity");
        let facet = string_or(value.get("facet"), "unknown");
        let participants = participants(value);
        lines.push(format!(
            "- {time} {title} [{activity}, {facet}]{}",
            if participants.is_empty() {
                String::new()
            } else {
                format!(" - participants: {participants}")
            }
        ));
    }
    lines.join("\n")
}
fn short_time(value: Option<&Value>) -> String {
    string_or(value, "").chars().take(5).collect()
}
fn participants(value: &Value) -> String {
    let mut names = value
        .get("participation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_object())
        .map(|entry| string_or(entry.get("name").or_else(|| entry.get("entity_id")), ""))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if names.is_empty() {
        names = value
            .get("active_entities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|entry| string_or(Some(entry), ""))
            .filter(|value| !value.is_empty())
            .collect();
    }
    names.join(", ")
}
fn render_search(values: &[SearchHit]) -> String {
    if values.is_empty() {
        "(none)".into()
    } else {
        values
            .iter()
            .map(|result| {
                format!(
                    "- {} [{}, {}]\n  {}",
                    result.id,
                    result.metadata.day,
                    result.metadata.facet,
                    result.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
fn coverage_preamble(
    counts: &Value,
    gaps: &[String],
    decisions_total: u64,
    forward: usize,
    followups_total: u64,
) -> String {
    let mut sentence = format!(
        "Built from {} indexed source paths, {} anticipated activities today, {forward} forward-looking anticipated activities, {} facet newsletters, {} follow-ups, {decisions_total} decision results.",
        counts["segments"],
        counts["anticipated_activities"],
        counts["facet_newsletters"],
        counts["followups"]
    );
    if followups_total > counts["followups"].as_u64().unwrap_or(0) {
        sentence.push_str(&format!(
            " Follow-up search returned {followups_total} total matches."
        ));
    }
    if gaps.is_empty() {
        sentence.push_str(" No gaps.");
    } else {
        sentence.push_str(&format!(" Gaps: {}.", gaps.join("; ")));
    }
    sentence
}
fn string_or(value: Option<&Value>, fallback: &str) -> String {
    value.and_then(Value::as_str).unwrap_or(fallback).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_keeps_reference_day_reasons() {
        // Derived from solstone/talent/morning_briefing.py:25-34.
        let prepared = PreparedTalent {
            name: "morning_briefing".into(),
            config: Map::new(),
        };
        let context = ExecutionContext {
            journal: Default::default(),
        };
        assert_eq!(
            gate(&prepared, &context).unwrap(),
            GateDecision::Skip("missing day".into())
        );
    }
}
