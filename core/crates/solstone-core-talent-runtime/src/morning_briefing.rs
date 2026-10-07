// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Morning-briefing pre-hook.

use std::collections::BTreeSet;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use serde_json::{Map, Value, json};
use solstone_core_facets::{load_activity_records, read_facet_declaration, read_news_file};
use solstone_core_home::{
    HomeContext,
    briefing::BriefingDates,
    readers::{enabled_facet_names, read_latest},
};
use solstone_core_profile_web::{read_owner_open_loops, types::LedgerItem};

use crate::contract::{GateDecision, PrePostState};
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
        prepared
            .config
            .get("_daily_upstream")
            .and_then(Value::as_object),
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

/// Apply the existing render before retrying an attention-count-only rejection.
/// Validate every raw row first so trimming cannot hide malformed discarded rows.
pub(crate) fn repair_attention_overflow(
    response: &mut solstone_core_generate::GeneratedResponse,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    if !crate::schema_validation_failed(response.schema_validation.as_ref()) {
        return Ok(());
    }
    let Some(schema) = prepared.config.get("json_schema") else {
        return Ok(());
    };
    let mut raw_schema = schema.clone();
    let Some(attention) = raw_schema
        .pointer_mut("/properties/needs_attention")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    if attention.remove("maxItems").is_none() {
        return Ok(());
    }
    let raw =
        solstone_core_generate_wire::validate_schema_with_annotations(&response.text, &raw_schema);
    if raw.validation["valid"] != true {
        return Ok(());
    }
    let rendered = preserve_open_loops(&response.text, prepared, state)?;
    let checked = solstone_core_generate_wire::validate_schema_with_annotations(&rendered, schema);
    if checked.validation["valid"] == true {
        response.text = rendered;
        response.schema_validation = Some(checked.validation);
    }
    Ok(())
}

/// Keep the two open-loop directions visible even when synthesis omits them.
/// Model-selected loop sources retain their order, but their text comes from the fold.
pub(crate) fn preserve_open_loops(
    output: &str,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<String, StageError> {
    let PrePostState::MorningBriefing(state) = state else {
        return Err(stage_error(
            "render",
            "morning_briefing",
            prepared,
            "missing briefing state",
        ));
    };
    let Some(loops) = state
        .values
        .get("briefing_open_loop_rows")
        .and_then(Value::as_array)
    else {
        return Ok(output.to_owned());
    };
    if loops.is_empty() {
        return Ok(output.to_owned());
    }
    let mut body: Value = serde_json::from_str(output)
        .map_err(|error| stage_error("render", "morning_briefing", prepared, error.to_string()))?;
    let rows = body
        .get_mut("needs_attention")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            stage_error(
                "render",
                "morning_briefing",
                prepared,
                "missing attention rows",
            )
        })?;
    let mut selected = Vec::new();
    // A source can hold several commitments. Publish the fold's selected item, never
    // infer which action or direction a paraphrase of that source meant.
    for row in rows.iter() {
        if let Some(item) = loops
            .iter()
            .find(|item| item["row"]["source_id"] == row["source_id"])
            && !selected.contains(&item)
        {
            selected.push(item);
        }
    }
    for owed in [true, false] {
        if !selected.iter().any(|item| item["owed"] == owed)
            && let Some(item) = loops.iter().find(|item| item["owed"] == owed)
        {
            selected.push(item);
        }
    }
    // Bound each direction separately so a voice-heavy direction cannot crowd out
    // the other. Leave eight of the existing twelve slots for generated day rows.
    selected.sort_by_key(|item| item["voice"] != true);
    let (mut owed, mut waiting) = (0, 0);
    selected.retain(|item| {
        let count = if item["owed"] == true {
            &mut owed
        } else {
            &mut waiting
        };
        *count += 1;
        *count <= 2
    });
    let mut combined = selected
        .iter()
        .map(|item| (item["voice"] == true, item["row"].clone()))
        .collect::<Vec<_>>();
    let voice_sources = state
        .values
        .get("briefing_voice_sources")
        .and_then(Value::as_array);
    let mut generated = rows
        .iter()
        .filter(|row| {
            !loops
                .iter()
                .any(|item| item["row"]["source_id"] == row["source_id"])
        })
        .map(|row| {
            (
                voice_sources.is_some_and(|sources| sources.contains(&row["source_id"])),
                row.clone(),
            )
        })
        .collect::<Vec<_>>();
    generated.sort_by_key(|(voice, _)| !voice);
    generated.truncate(12 - combined.len());
    combined.extend(generated);
    combined.sort_by_key(|(voice, _)| !voice);
    *rows = combined.into_iter().take(12).map(|(_, row)| row).collect();
    let rendered = serde_json::to_string(&body)
        .map_err(|error| stage_error("render", "morning_briefing", prepared, error.to_string()))?;
    if let Some(schema) = prepared.config.get("json_schema") {
        let checked =
            solstone_core_generate_wire::validate_schema_with_annotations(&rendered, schema);
        if checked.validation["valid"] != true {
            return Err(stage_error(
                "render",
                "morning_briefing",
                prepared,
                "briefing rows do not fit the output schema",
            ));
        }
    }
    Ok(rendered)
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
    upstream: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, String> {
    let dates = BriefingDates::for_analysis(analysis_day).ok_or("briefing date overflow")?;
    let presentation_day = dates.presentation.format("%Y%m%d").to_string();
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
    let newsletter_facets = facets
        .iter()
        .filter(|(facet, _)| {
            upstream.is_none_or(|outcomes| {
                outcomes.get(&format!("facet_newsletter:{facet}")) == Some(&Value::Bool(true))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let newsletters = load_newsletters(&newsletter_facets, day, context, &mut gaps);
    let schedule_current =
        upstream.is_none_or(|outcomes| outcomes.get("schedule") == Some(&Value::Bool(true)));
    let schedule_facets = if schedule_current {
        facets.as_slice()
    } else {
        gaps.push("current schedule unavailable".to_owned());
        &[]
    };
    let today = load_activities(
        schedule_facets,
        std::slice::from_ref(&presentation_day),
        context,
        &mut gaps,
        "no anticipated activities today",
    );
    let forward_days = (1..8)
        .map(|offset| {
            (dates.presentation + Duration::days(offset))
                .format("%Y%m%d")
                .to_string()
        })
        .collect::<Vec<_>>();
    let forward = load_activities(
        schedule_facets,
        &forward_days,
        context,
        &mut gaps,
        "no anticipated activities in the next 7 days",
    );
    let (followups_total, followups) = load_story_items(
        &facets,
        day,
        "commitments",
        "follow-up items",
        context,
        &mut gaps,
    );
    let (decisions_total, decisions) = load_story_items(
        &facets,
        day,
        "decisions",
        "decision items",
        context,
        &mut gaps,
    );
    let (loops_total, loops) = load_open_loops(
        &facets,
        day,
        dates.presentation,
        &home,
        &followups,
        &mut gaps,
    );
    let pulse = read_pulse(&home, day, &mut gaps);
    let mut paths = followups
        .iter()
        .chain(&decisions)
        .map(StoryItem::source)
        .collect::<BTreeSet<_>>();
    paths.extend(loops.iter().flat_map(|(item, _)| {
        item.sources.iter().map(|source| {
            format!(
                "facets/{}/activities/{}.jsonl#{}",
                source.facet, source.day, source.activity_id
            )
        })
    }));
    let counts = json!({"segments": paths.len(), "anticipated_activities": today.len(), "facet_newsletters": newsletters.len(), "followups": followups.len() + loops.len()});
    let metadata = json!({"generated": generated_stamp(&home), "model": model, "sources": counts, "gaps": gaps, "coverage_preamble": coverage_preamble(&counts, &gaps, decisions_total, forward.len(), followups_total + loops_total)});
    Ok(Map::from_iter([
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
            "followups".into(),
            Value::String(render_followups(&followups, &loops, dates.presentation)),
        ),
        (
            "briefing_open_loop_rows".into(),
            Value::Array(loops.iter().filter_map(|(item, owed)| {
                let source = item.sources.iter().find(|source| source.field == "commitments")?;
                let source_id = format!("sol://facets/{}/activities/{}/{}", source.facet, source.day, source.activity_id);
                if source_id.chars().count() > 240 { return None; }
                let age = NaiveDate::parse_from_str(&source.day, "%Y%m%d").ok()
                    .map(|opened| (dates.presentation - opened).num_days())?;
                let prefix = if *owed {
                    format!("what you owe: {}", item.action)
                } else {
                    format!("what you're waiting on: {} to {}", item.owner, item.action)
                };
                let suffix = format!("; still open for {age} days (opened {}).", source.day);
                let text = format!("{}{}", prefix.chars().take(700 - suffix.chars().count()).collect::<String>(), suffix);
                Some(json!({"owed":owed,"voice":item.owner_evidence.as_deref()==Some("voice"),"row":{"text":text,"source_id":source_id}}))
            }).collect()),
        ),
        (
            "briefing_voice_sources".into(),
            Value::Array(followups.iter().filter(|item| item.said_by_you()).map(|item| {
                json!(format!("sol://facets/{}/activities/{}/{}", item.facet, item.day, item.record_id))
            }).collect()),
        ),
        (
            "decisions".into(),
            Value::String(render_story_items(&decisions)),
        ),
    ]))
}

/// Add older open loops without changing the analysis day's all-actor selection.
/// Each direction gets its own cut, with recognized voice first, then newest opening.
fn load_open_loops(
    facets: &[(String, String)],
    day: &str,
    presentation: NaiveDate,
    home: &HomeContext,
    daily: &[StoryItem],
    gaps: &mut Vec<String>,
) -> (u64, Vec<(LedgerItem, bool)>) {
    let result = (|| {
        let principal = crate::JournalOwner::load(&home.journal_root)?
            .id
            .ok_or("principal unavailable")?;
        let as_of = home
            .zone()
            .from_local_datetime(
                &presentation
                    .and_hms_opt(0, 0, 0)
                    .ok_or("invalid presentation date")?,
            )
            .earliest()
            .ok_or("presentation date unavailable")?
            .with_timezone(&Utc);
        let names = facets
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let items = read_owner_open_loops(&home.journal_root, &principal, day, as_of, &names)?;
        Ok::<_, String>((principal, items))
    })();
    let (principal, items) = match result {
        Ok(value) => value,
        Err(error) => {
            log::warn!("briefing open loops unavailable: {error}");
            gaps.push("open loops unavailable".into());
            return (0, Vec::new());
        }
    };
    let mut items = items
        .into_iter()
        .filter(|item| {
            item.sources
                .iter()
                .find(|source| source.field == "commitments")
                .is_some_and(|source| source.day.as_str() < day)
                && !daily.iter().any(|new| {
                    normalize_action(&item.action)
                        == normalize_action(&string_or(new.item.get("action"), ""))
                        && item.sources.iter().any(|source| {
                            source.field == "commitments"
                                && source.day == new.day
                                && source.facet == new.facet
                                && source.activity_id == new.record_id
                        })
                })
        })
        .map(|item| {
            let owed = item.owner_entity_id.as_deref() == Some(principal.as_str());
            (item, owed)
        })
        .collect::<Vec<_>>();
    let total = items.len() as u64;
    items.sort_by_key(|(item, _)| {
        (
            item.owner_evidence.as_deref() != Some("voice"),
            std::cmp::Reverse(item.opened_at),
            item.id.clone(),
        )
    });
    let mut owed = 0;
    let mut waiting = 0;
    items.retain(|(_, yours)| {
        let count = if *yours { &mut owed } else { &mut waiting };
        *count += 1;
        *count <= 10
    });
    (total, items)
}

fn normalize_action(action: &str) -> String {
    action
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_followups(
    daily: &[StoryItem],
    loops: &[(LedgerItem, bool)],
    presentation: NaiveDate,
) -> String {
    let mut lines = daily
        .iter()
        .map(|item| {
            (
                item.said_by_you(),
                render_story_items(std::slice::from_ref(item)),
            )
        })
        .collect::<Vec<_>>();
    lines.extend(loops.iter().map(|(item, owed)| {
        let source = item
            .sources
            .iter()
            .find(|source| source.field == "commitments")
            .expect("open loop has an opening");
        let age = NaiveDate::parse_from_str(&source.day, "%Y%m%d")
            .ok()
            .map(|opened| (presentation - opened).num_days());
        let voice = item.owner_evidence.as_deref() == Some("voice");
        let direction = if *owed {
            "what you owe"
        } else {
            "what you're waiting on"
        };
        let mut line = format!(
            "- {}: {}; actor: {}; opened {}",
            direction, item.action, item.owner, source.day
        );
        if let Some(age) = age {
            line.push_str(&format!("; open for {age} days"));
        }
        if let Some(when) = &item.when {
            line.push_str(&format!("; stated timing: {when}"));
        }
        if voice {
            line.push_str("; said by you");
        }
        line.push_str(&format!(
            "\n  Source: sol://facets/{}/activities/{}/{}",
            source.facet, source.day, source.activity_id
        ));
        if !item.context.trim().is_empty() {
            line.push_str(&format!("\n  {}", item.context));
        }
        (voice, line)
    }));
    lines.sort_by_key(|(voice, _)| !voice);
    if lines.is_empty() {
        "(none)".into()
    } else {
        lines
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    }
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
/// One commitment or decision an activity's story saved on its record.
struct StoryItem {
    facet: String,
    day: String,
    record_id: String,
    title: String,
    item: Map<String, Value>,
}

impl StoryItem {
    fn said_by_you(&self) -> bool {
        self.item.get("owner_evidence").and_then(Value::as_str) == Some("voice")
    }

    fn source(&self) -> String {
        format!(
            "facets/{}/activities/{}.jsonl#{}",
            self.facet, self.day, self.record_id
        )
    }
}

/// Read the `key` array (`commitments` or `decisions`) that activity stories
/// saved on the day's activity records, across the enabled facets.
fn load_story_items(
    facets: &[(String, String)],
    day: &str,
    key: &str,
    label: &str,
    context: &ExecutionContext,
    gaps: &mut Vec<String>,
) -> (u64, Vec<StoryItem>) {
    let mut items = Vec::new();
    for (facet, _) in facets {
        match load_activity_records(&context.journal, facet, day, false) {
            Ok(records) => {
                for record in records {
                    let saved = record
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_object)
                        .filter(|item| !string_or(item.get("action"), "").trim().is_empty());
                    for item in saved {
                        items.push(StoryItem {
                            facet: facet.clone(),
                            day: day.to_owned(),
                            record_id: string_or(record.get("id"), ""),
                            title: string_or(
                                record.get("title").or_else(|| record.get("activity")),
                                "",
                            ),
                            item: item.clone(),
                        });
                    }
                }
            }
            Err(error) => gaps.push(format!("{label} unavailable for {facet}: {error}")),
        }
    }
    if items.is_empty() {
        gaps.push(format!("no {label} found"));
    }
    let total = items.len() as u64;
    // What the owner said, by their recognized voice, comes first; the order is otherwise kept.
    items.sort_by_key(|item| !item.said_by_you());
    items.truncate(10);
    (total, items)
}
/// When the briefing was prepared, as a wall time on the journal's clock; home
/// shows its hour and minute as written.
fn generated_stamp(home: &HomeContext) -> String {
    home.now_local().format("%Y-%m-%dT%H:%M:%S").to_string()
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
fn render_story_items(values: &[StoryItem]) -> String {
    if values.is_empty() {
        return "(none)".into();
    }
    values
        .iter()
        .map(|value| {
            let field = |name: &str| string_or(value.item.get(name), "").trim().to_owned();
            let mut line = format!("- {}", field("action"));
            for (label, name) in [
                ("owner", "owner"),
                ("with", "counterparty"),
                ("when", "when"),
            ] {
                let text = field(name);
                if !text.is_empty() {
                    line.push_str(&format!("; {label}: {text}"));
                }
            }
            if value.said_by_you() {
                line.push_str("; said by you");
            }
            line.push_str(&format!(" [{}, {}", value.day, value.facet));
            if !value.title.is_empty() {
                line.push_str(&format!(", {}", value.title));
            }
            line.push(']');
            line.push_str(&format!(
                "\n  Source: sol://facets/{}/activities/{}/{}",
                value.facet, value.day, value.record_id
            ));
            let context = field("context");
            if !context.is_empty() {
                line.push_str(&format!("\n  {context}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn coverage_preamble(
    counts: &Value,
    gaps: &[String],
    decisions_total: u64,
    forward: usize,
    followups_total: u64,
) -> String {
    let mut sentence = format!(
        "Built from {} source paths, {} anticipated activities today, {forward} forward-looking anticipated activities, {} facet newsletters, {} follow-ups, {decisions_total} decision results.",
        counts["segments"],
        counts["anticipated_activities"],
        counts["facet_newsletters"],
        counts["followups"]
    );
    if followups_total > counts["followups"].as_u64().unwrap_or(0) {
        sentence.push_str(&format!(
            " Follow-up sources contain {followups_total} total matches."
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
    fn attention_overflow_is_rendered_without_hiding_other_schema_errors() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../payload/solstone/talent/morning_briefing.schema.json"
        ))
        .unwrap();
        let prepared = PreparedTalent {
            name: "morning_briefing".into(),
            config: Map::from_iter([("json_schema".into(), schema.clone())]),
        };
        let loops = (0..20)
            .map(|n| {
                json!({"owed":n < 10,"voice":n == 0,"row":{
                    "text":format!("still open for {} days", n + 1),
                    "source_id":format!("sol://older/{n}")
                }})
            })
            .collect::<Vec<_>>();
        let state = PrePostState::MorningBriefing(MorningBriefingPreState {
            values: Map::from_iter([("briefing_open_loop_rows".into(), json!(loops))]),
        });
        let base = json!({
            "metadata":{"generated":"2026-10-07T00:00:00Z","model":"test",
                "sources":{"segments":0,"anticipated_activities":0,"facet_newsletters":0,"followups":20},
                "gaps":[],"coverage_preamble":""},
            "your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]
        });
        let decode = |body: &Value| {
            let text = body.to_string();
            let checked =
                solstone_core_generate_wire::validate_schema_with_annotations(&text, &schema);
            let crate::GenerateResponse::Generated(response) =
                solstone_core_generate::decode_one_shot_response(
                    &crate::test_support::generated_response_value(&text, checked.validation)
                        .to_string(),
                )
                .unwrap()
            else {
                panic!("expected generated response")
            };
            response
        };
        for count in [13, 20] {
            let mut body = base.clone();
            body["needs_attention"] = json!(
                loops
                    .iter()
                    .take(count)
                    .map(|item| item["row"].clone())
                    .collect::<Vec<_>>()
            );
            let mut response = decode(&body);
            assert!(crate::schema_validation_failed(
                response.schema_validation.as_ref()
            ));
            repair_attention_overflow(&mut response, &prepared, &state).unwrap();
            assert!(!crate::schema_validation_failed(
                response.schema_validation.as_ref()
            ));
            let rendered: Value = serde_json::from_str(&response.text).unwrap();
            assert_eq!(rendered["needs_attention"].as_array().unwrap().len(), 4);
            assert_eq!(rendered["needs_attention"][0]["source_id"], "sol://older/0");
            assert_eq!(
                rendered["needs_attention"][2]["source_id"],
                "sol://older/10"
            );
        }

        let mut crowded = base.clone();
        crowded["needs_attention"] = json!(
            (0..13)
                .map(|n| json!({"text":"day task","source_id":format!("sol://day/{n}")}))
                .collect::<Vec<_>>()
        );
        let mut response = decode(&crowded);
        repair_attention_overflow(&mut response, &prepared, &state).unwrap();
        let rendered: Value = serde_json::from_str(&response.text).unwrap();
        assert_eq!(rendered["needs_attention"].as_array().unwrap().len(), 12);

        let mut malformed_tail = crowded.clone();
        malformed_tail["needs_attention"][12]["text"] = json!(17);
        let mut malformed_loop = crowded.clone();
        malformed_loop["needs_attention"][0] = json!({"text":17,"source_id":"sol://older/0"});
        let mut missing_metadata = crowded.clone();
        missing_metadata.as_object_mut().unwrap().remove("metadata");
        let mut other_overflow = crowded.clone();
        other_overflow["yesterday"] = json!(vec!["day"; 11]);
        for invalid in [
            malformed_tail,
            malformed_loop,
            missing_metadata,
            other_overflow,
            json!([]),
        ] {
            let mut response = decode(&invalid);
            repair_attention_overflow(&mut response, &prepared, &state).unwrap();
            assert_eq!(response.text, invalid.to_string());
            assert!(crate::schema_validation_failed(
                response.schema_validation.as_ref()
            ));
        }
        let empty_state =
            PrePostState::MorningBriefing(MorningBriefingPreState { values: Map::new() });
        let mut response = decode(&crowded);
        repair_attention_overflow(&mut response, &prepared, &empty_state).unwrap();
        assert!(crate::schema_validation_failed(
            response.schema_validation.as_ref()
        ));
    }
    use std::fs;

    #[test]
    fn briefing_adds_owner_directions_with_age_and_keeps_new_items_all_actor() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(
            facet.join("facet.json"),
            json!({"title":"Work"}).to_string(),
        )
        .unwrap();
        for (id, name, principal) in [("owner", "Jordan", true), ("pat", "Pat", false)] {
            let dir = root.path().join("entities").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("entity.json"),
                json!({"id":id,"name":name,"type":"Person","is_principal":principal}).to_string(),
            )
            .unwrap();
        }
        fs::write(facet.join("activities/20260701.jsonl"), json!({"id":"old","created_at":1782864000000_i64,"commitments":[
            {"owner":"you","owner_entity_id":"owner","action":"send report","owner_evidence":"voice"},
            {"owner":"Pat","owner_entity_id":"pat","counterparty_entity_id":"owner","action":"send estimate"},
            {"owner":"your agent","action":"agent decoy"}
        ]}).to_string()).unwrap();
        fs::write(facet.join("activities/20261003.jsonl"), json!({"id":"new","created_at":1790985600000_i64,"commitments":[{"owner":"Someone","action":"new item"}]}).to_string()).unwrap();
        fs::write(facet.join("activities/20261004.jsonl"), json!({"id":"future","created_at":1791072000000_i64,"closures":[{"owner_entity_id":"owner","action":"send report","resolution":"done"}]}).to_string()).unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let values = build_packet(
            "20261003",
            NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            "test",
            &context,
            None,
        )
        .unwrap();
        let text = values["followups"].as_str().unwrap();
        assert!(text.starts_with("- what you owe: send report"), "{text}");
        assert!(
            text.contains("what you're waiting on: send estimate"),
            "{text}"
        );
        assert!(text.contains("open for 95 days"), "{text}");
        assert!(
            text.contains("new item") && !text.contains("decoy"),
            "{text}"
        );
        assert!(text.contains("sol://facets/work/activities/20260701/old"));
    }

    #[test]
    fn missing_directions_are_published_with_age_and_voice_first() {
        let prepared = PreparedTalent {
            name: "morning_briefing".into(),
            config: Map::new(),
        };
        let mut loops = (0..10).map(|n| json!({"owed":true,"voice":true,"row":{"text":format!("what you owe: task {n}; still open for 90 days"),"source_id":format!("sol://owner/{n}")}})).collect::<Vec<_>>();
        loops.push(json!({"owed":false,"voice":false,"row":{"text":"what you're waiting on: Pat to send report; still open for 90 days","source_id":"sol://waiting/1"}}));
        let state = PrePostState::MorningBriefing(MorningBriefingPreState {
            values: Map::from_iter([
                ("briefing_open_loop_rows".into(), json!(loops)),
                (
                    "briefing_voice_sources".into(),
                    json!(["sol://daily/voice"]),
                ),
            ]),
        });
        let output = json!({"needs_attention":[{"text":"new voice task","source_id":"sol://daily/voice"}],"your_day":[]}).to_string();
        let result: Value =
            serde_json::from_str(&preserve_open_loops(&output, &prepared, &state).unwrap())
                .unwrap();
        let rows = result["needs_attention"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["source_id"], "sol://owner/0");
        assert_eq!(rows[1]["source_id"], "sol://daily/voice");
        assert_eq!(rows[2]["source_id"], "sol://waiting/1");
        let ranked = json!({"needs_attention": (0..10).map(|n| json!({"text":"misleading urgency","source_id":format!("sol://owner/{n}")})).collect::<Vec<_>>()}).to_string();
        let result: Value =
            serde_json::from_str(&preserve_open_loops(&ranked, &prepared, &state).unwrap())
                .unwrap();
        let rows = result["needs_attention"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|row| row["source_id"] == "sol://waiting/1"));
        assert!(
            rows.iter()
                .all(|row| row["text"].as_str().unwrap().contains("open for 90 days"))
        );
        let crowded = json!({"needs_attention": (0..12).map(|_| json!({"text":"new voice task","source_id":"sol://daily/voice"})).collect::<Vec<_>>()}).to_string();
        let result: Value =
            serde_json::from_str(&preserve_open_loops(&crowded, &prepared, &state).unwrap())
                .unwrap();
        let rows = result["needs_attention"].as_array().unwrap();
        assert_eq!(rows.len(), 12);
        assert!(rows.iter().any(|row| row["source_id"] == "sol://owner/0"));
        assert_eq!(rows.last().unwrap()["source_id"], "sol://waiting/1");
    }

    #[test]
    fn unavailable_ledger_contributes_no_loops_and_reports_the_gap() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("entities"), "unreadable entity directory").unwrap();
        let home = HomeContext::new(root.path(), Utc::now());
        let mut gaps = Vec::new();
        let (total, loops) = load_open_loops(
            &[],
            "20261003",
            NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
            &home,
            &[],
            &mut gaps,
        );
        assert_eq!(total, 0);
        assert!(loops.is_empty());
        assert_eq!(gaps.len(), 1);
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
    fn packet_keeps_analysis_sources_but_uses_the_next_mornings_agenda() {
        let root = tempfile::TempDir::new().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::create_dir_all(facet.join("news")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        for (day, title) in [
            ("20261231", "analysis agenda decoy"),
            ("20270101", "presentation agenda"),
            ("20270102", "forward agenda"),
        ] {
            fs::write(
                facet.join(format!("activities/{day}.jsonl")),
                json!({"id":day,"source":"anticipated","title":title}).to_string(),
            )
            .unwrap();
        }
        fs::write(facet.join("news/20261231.md"), "analysis newsletter").unwrap();
        fs::write(
            facet.join("news/20270101.md"),
            "presentation newsletter decoy",
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let values = build_packet(
            "20261231",
            NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            "test",
            &context,
            None,
        )
        .unwrap();
        assert_eq!(values["briefing_analysis_day"], "20261231");
        assert_eq!(values["briefing_presentation_day"], "20270101");
        let today = values["anticipated_today"].as_str().unwrap();
        assert!(today.contains("presentation agenda"), "{today}");
        assert!(!today.contains("decoy") && !today.contains("forward agenda"));
        let forward = values["anticipated_forward"].as_str().unwrap();
        assert!(forward.contains("forward agenda"), "{forward}");
        assert!(!forward.contains("presentation agenda") && !forward.contains("decoy"));
        let news = values["facet_newsletters"].as_str().unwrap();
        assert!(news.contains("analysis newsletter"), "{news}");
        assert!(!news.contains("decoy"));
    }

    #[test]
    fn what_you_said_in_your_own_voice_leads_the_follow_ups() {
        let root = tempfile::TempDir::new().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        let mut rows = (0..11)
            .map(|n| json!({"id":format!("screen-{n}"),"commitments":[{"owner":"you","action":format!("screen item {n}")}]}))
            .collect::<Vec<_>>();
        rows.push(json!({"id":"call","title":"Call","commitments":[{"owner":"you","action":"send the deck","owner_evidence":"voice"}]}));
        fs::write(
            facet.join("activities/20260910.jsonl"),
            rows.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let values = build_packet("20260910", date, "test", &context, None).unwrap();
        let followups = values["followups"].as_str().unwrap();
        assert!(
            followups.starts_with("- send the deck; owner: you; said by you"),
            "{followups}"
        );
        assert!(followups.contains("screen item 0") && !followups.contains("screen item 9"));
    }

    #[test]
    fn follow_ups_and_decisions_come_from_the_analysis_days_activity_records() {
        let root = tempfile::TempDir::new().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        fs::write(
            facet.join("activities/20260910.jsonl"),
            [
                json!({"id":"planning","title":"Planning","commitments":[{"owner":"You","action":"send the launch brief","counterparty":"Pat","when":"Friday","context":"agreed in planning"}],"decisions":[{"owner":"You","action":"ship on Monday","counterparty":null,"context":"after review"}]}),
                json!({"id":"quiet","title":"Reading"}),
            ]
            .map(|row| row.to_string())
            .join("\n"),
        )
        .unwrap();
        fs::write(
            facet.join("activities/20260911.jsonl"),
            json!({"id":"later","commitments":[{"owner":"You","action":"other day decoy"}]})
                .to_string(),
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let values = build_packet("20260910", date, "test", &context, None).unwrap();
        let followups = values["followups"].as_str().unwrap();
        assert!(followups.contains("send the launch brief"), "{followups}");
        assert!(followups.contains("Pat") && followups.contains("Friday"));
        assert!(!followups.contains("decoy"));
        let decisions = values["decisions"].as_str().unwrap();
        assert!(decisions.contains("ship on Monday"), "{decisions}");
        assert!(!decisions.contains("send the launch brief"));
        let metadata: Value =
            serde_json::from_str(values["briefing_metadata"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["sources"]["followups"], 1);
        assert_eq!(metadata["sources"]["segments"], 1);
        let gaps = metadata["gaps"].to_string();
        assert!(!gaps.contains("follow-up items") && !gaps.contains("decision items"));

        // A day whose stories saved nothing reports the gap instead of a count.
        let empty = build_packet(
            "20260912",
            NaiveDate::from_ymd_opt(2026, 9, 12).unwrap(),
            "test",
            &context,
            None,
        )
        .unwrap();
        assert_eq!(empty["followups"], "(none)");
        assert_eq!(empty["decisions"], "(none)");
    }

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
    #[test]
    fn briefing_never_lends_retained_news_or_calendar_after_no_output_then_accepts_recovery() {
        let root = tempfile::tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(facet.join("news")).unwrap();
        fs::create_dir_all(facet.join("activities")).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        fs::write(facet.join("news/20260910.md"), "Retained OLD newsletter").unwrap();
        fs::write(
            facet.join("activities/20260911.jsonl"),
            r#"{"id":"old","source":"anticipated","title":"Retained OLD agenda"}"#,
        )
        .unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let unavailable = Map::from_iter([
            ("schedule".to_owned(), json!(false)),
            ("facet_newsletter:work".to_owned(), json!(false)),
        ]);
        let values = build_packet("20260910", date, "test", &context, Some(&unavailable)).unwrap();
        let text = serde_json::to_string(&values).unwrap();
        assert!(!text.contains("Retained OLD"));
        fs::write(facet.join("news/20260910.md"), "Recovered NEW newsletter").unwrap();
        let available = Map::from_iter([
            ("schedule".to_owned(), json!(true)),
            ("facet_newsletter:work".to_owned(), json!(true)),
        ]);
        let values = build_packet("20260910", date, "test", &context, Some(&available)).unwrap();
        assert!(
            serde_json::to_string(&values)
                .unwrap()
                .contains("Recovered NEW newsletter")
        );
    }
}
