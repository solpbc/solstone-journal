// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Filesystem and native-store readers. This module performs no writes.
//!
//! It remains a single reader module until the pure projections are added as
//! separate modules. Those projections will have neither filesystem imports
//! nor a journal-root input, which makes their no-I/O boundary explicit.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection};
use solstone_core_entities::{ATTENDANCE_KINDS, ENTITIES_COPY};
use solstone_core_facets::{
    list_declared_facet_names, load_activity_records, load_current, read_facet_declaration,
};
use solstone_core_indexer_query::{NetworkRequest, load_entity_network};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, catalog_oplogs},
};
use solstone_core_journal_stats_cli::estimate_duration_minutes;
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientCaptureState, ClientInspection,
    ConnectionFreshness, SourceDelivery, inspect_clients_at, rollup_client_capture_states,
};
use solstone_core_system_health::{FilesystemHealthLogSource, TerminalEvent, read_terminal_states};

use crate::HomeContext;
use crate::formatting::format_date;
use crate::model::{BacklogSource, BacklogValidity, FlowDocument, PulseNarrative};

const BRIEFING_MORNING_END_HOUR: u32 = 10;
const BRIEFING_LATENESS_THRESHOLD_HOURS: u32 = 2;
const BRIEFING_EOD_HOUR: u32 = 20;

/// Count elapsed calendar days from the earliest valid chronicle directory.
pub fn count_journal_age_days(context: &HomeContext) -> i64 {
    let chronicle = context.journal_root().join("chronicle");
    let Ok(entries) = fs::read_dir(chronicle) else {
        return 0;
    };
    let earliest = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry.file_type().ok().filter(|kind| kind.is_dir())?;
            let day = entry.file_name().to_string_lossy().into_owned();
            (day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| NaiveDate::parse_from_str(&day, "%Y%m%d").ok())
                .flatten()
        })
        .min();
    earliest
        .map(|day| (context.now_utc.date_naive() - day).num_days().max(0))
        .unwrap_or(0)
}

/// Read the newest valid weekly reflection. The returned object intentionally has no URL.
pub fn load_latest_weekly_reflection(context: &HomeContext) -> Option<Value> {
    let directory = context.journal_root().join("reflections/weekly");
    let mut days = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry.file_type().ok().filter(|kind| kind.is_file())?;
            let stem = entry.path().file_stem()?.to_str()?.to_owned();
            (stem.len() == 8 && stem.bytes().all(|byte| byte.is_ascii_digit())).then_some(stem)
        })
        .collect::<Vec<_>>();
    days.sort();
    let day = days.pop()?;
    Some(json!({"day": day, "label": format_date(&day)}))
}

/// Read `chronicle/<day>/talents/flow.md` without creating its parent directories.
pub fn load_flow_md(context: &HomeContext, day: &str) -> FlowDocument {
    let path = day_root(context, day).join("talents/flow.md");
    match fs::read_to_string(&path) {
        Ok(content) => FlowDocument {
            content: Some(content),
            updated_at: path
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs_f64()),
        },
        Err(_) => FlowDocument {
            content: None,
            updated_at: None,
        },
    }
}

/// Read the newest valid day-accumulator record for a name, skipping malformed JSONL rows.
pub fn read_latest(
    context: &HomeContext,
    day: &str,
    name: &str,
    lookback_days: u32,
) -> Option<Value> {
    let start = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    for offset in 0..=lookback_days {
        let probe = (start - Duration::days(i64::from(offset)))
            .format("%Y%m%d")
            .to_string();
        let path = day_root(context, &probe)
            .join("talents")
            .join(format!("{name}.jsonl"));
        let rows = read_jsonl_objects(&path);
        if !rows.is_empty() {
            return rows
                .into_iter()
                .enumerate()
                .max_by_key(|(index, row)| {
                    (row.get("ts").and_then(Value::as_i64).unwrap_or(0), *index)
                })
                .map(|(_, row)| Value::Object(row));
        }
    }
    None
}

/// Read today's pulse narrative from the pulse accumulator.
pub fn load_pulse_narrative(context: &HomeContext, day: &str) -> PulseNarrative {
    let Some(record) = read_latest(context, day, "pulse", 0) else {
        return empty_pulse();
    };
    let Some(content) = record
        .get("full_details")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return empty_pulse();
    };
    let needs = record
        .get("needs_you")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| match value {
            Value::String(value) => value.to_owned(),
            value => value.to_string(),
        })
        .filter(|value| !value.trim_matches('"').trim().is_empty())
        .collect();
    let updated_at = record
        .get("ts")
        .and_then(Value::as_i64)
        .and_then(DateTime::from_timestamp_millis)
        .map(|time| time.with_timezone(&Utc).format("%H:%M").to_string());
    PulseNarrative {
        content: Some(content.to_owned()),
        updated_at,
        needs,
    }
}

/// Raw day stats from the corrected chronicle path; no freshness policy is applied.
pub fn load_stats(context: &HomeContext, day: &str) -> Value {
    read_json_value(&day_root(context, day).join("stats.json")).unwrap_or_else(|| json!({}))
}

/// Raw prior-day stats from the corrected chronicle path.
pub fn load_yesterday_stats(context: &HomeContext) -> Option<Value> {
    read_json_value(&day_root(context, &context.yesterday()).join("stats.json"))
}

/// Return declared facets, including muted facets and excluding directories without `facet.json`.
pub fn all_facet_names(context: &HomeContext) -> Vec<String> {
    list_declared_facet_names(context.journal_root()).unwrap_or_default()
}

/// Return declared facets whose declaration is not muted.
pub fn enabled_facet_names(context: &HomeContext) -> Vec<String> {
    all_facet_names(context)
        .into_iter()
        .filter(|facet| {
            read_facet_declaration(context.journal_root(), facet)
                .ok()
                .flatten()
                .is_none_or(|declaration| declaration.muted != Some(true))
        })
        .collect()
}

/// Collect anticipated activity records across every declared facet.
pub fn collect_anticipated_activities(context: &HomeContext, day: &str) -> Vec<Value> {
    all_facet_names(context).into_iter().flat_map(|facet| {
        load_activity_records(context.journal_root(), &facet, day, true).unwrap_or_default().into_iter().filter_map(move |record| {
            (record.get("source").and_then(Value::as_str) == Some("anticipated")).then(|| {
                let participants = record.get("participation").and_then(Value::as_array).into_iter().flatten().filter_map(|entry| {
                    (entry.get("role").and_then(Value::as_str) == Some("attendee")).then(|| entry.get("name").and_then(Value::as_str).unwrap_or("").trim().to_owned()).filter(|name| !name.is_empty())
                }).collect::<Vec<_>>();
                json!({"title": record.get("title").cloned().unwrap_or(Value::String(String::new())), "start": record.get("start").cloned().unwrap_or(Value::String(String::new())), "end": record.get("end").cloned().unwrap_or(Value::String(String::new())), "facet": facet.clone(), "occurred": false, "participants": participants})
            })
        })
    }).collect()
}

/// Collect non-anticipated activity records created within four hours of the injected instant.
pub fn collect_activities(context: &HomeContext, day: &str) -> Vec<Value> {
    let cutoff = context.now_ms() - 4 * 60 * 60 * 1000;
    let mut rows = all_facet_names(context)
        .into_iter()
        .flat_map(|facet| {
            load_activity_records(context.journal_root(), &facet, day, true)
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |record| {
                    let created = record
                        .get("created_at")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    (record.get("source").and_then(Value::as_str) != Some("anticipated")
                        && created >= cutoff)
                        .then(|| {
                            let mut record = record;
                            record.insert(
                                "display_time".to_owned(),
                                DateTime::from_timestamp_millis(created)
                                    .map(|time| {
                                        time.with_timezone(&Utc).format("%H:%M").to_string()
                                    })
                                    .unwrap_or_default()
                                    .into(),
                            );
                            record.insert("facet".to_owned(), facet.clone().into());
                            Value::Object(record)
                        })
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        std::cmp::Reverse(row.get("created_at").and_then(Value::as_i64).unwrap_or(0))
    });
    rows
}

/// Collect enabled-facet activity records and use the native duration estimator.
pub fn collect_top_activities_yesterday(context: &HomeContext) -> Vec<Value> {
    let day = context.yesterday();
    let mut rows = enabled_facet_names(context)
        .into_iter()
        .flat_map(|facet| {
            load_activity_records(context.journal_root(), &facet, &day, true)
                .unwrap_or_default()
                .into_iter()
                .map(move |mut record| {
                    let segments = record
                        .get("segments")
                        .and_then(Value::as_array)
                        .map(|rows| {
                            rows.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let title = record
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .map(str::to_owned)
                        .or_else(|| {
                            record
                                .get("activity")
                                .and_then(Value::as_str)
                                .filter(|text| !text.trim().is_empty())
                                .map(|text| title_case(&text.replace('_', " ")))
                        })
                        .unwrap_or_else(|| "untitled activity".to_owned());
                    record.insert("facet".to_owned(), facet.clone().into());
                    record.insert("title".to_owned(), title.into());
                    record.insert(
                        "duration_minutes".to_owned(),
                        estimate_duration_minutes(&segments).into(),
                    );
                    Value::Object(record)
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .get("duration_minutes")
            .and_then(Value::as_u64)
            .cmp(&left.get("duration_minutes").and_then(Value::as_u64))
            .then_with(|| {
                left.get("title")
                    .and_then(Value::as_str)
                    .cmp(&right.get("title").and_then(Value::as_str))
            })
            .then_with(|| {
                left.get("facet")
                    .and_then(Value::as_str)
                    .cmp(&right.get("facet").and_then(Value::as_str))
            })
    });
    rows
}

/// Canonical morning-briefing JSON path.
pub fn morning_briefing_path(context: &HomeContext, day: &str) -> std::path::PathBuf {
    day_root(context, day).join("talents/morning_briefing.json")
}

/// Load only a briefing document with the required root keys.
pub fn load_briefing(context: &HomeContext, day: &str) -> Option<Value> {
    let briefing = read_json_value(&morning_briefing_path(context, day))?;
    let object = briefing.as_object()?;
    [
        "metadata",
        "your_day",
        "yesterday",
        "needs_attention",
        "forward_look",
        "reading",
    ]
    .iter()
    .all(|key| object.contains_key(*key))
    .then_some(briefing)
}

/// Render the non-empty briefing sections without reading the filesystem.
pub fn render_briefing_sections(briefing: &Value) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let strings = |key: &str| {
        briefing
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| format!("- {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    for key in ["yesterday", "forward_look"] {
        let text = strings(key);
        if !text.is_empty() {
            sections.insert(key.to_owned(), text);
        }
    }
    for (key, fields) in [
        ("your_day", ("time", "text")),
        ("reading", ("facet", "summary")),
    ] {
        let rows = briefing
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter_map(|row| {
                let left = row
                    .get(fields.0)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let right = row
                    .get(fields.1)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                (!left.is_empty() || !right.is_empty()).then(|| {
                    match (left.is_empty(), right.is_empty()) {
                        (false, false) => format!("- **{left}** — {right}"),
                        (false, true) => format!("- **{left}**"),
                        (true, false) => format!("- {right}"),
                        _ => String::new(),
                    }
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !rows.is_empty() {
            sections.insert(key.to_owned(), rows);
        }
    }
    let needs = briefing_needs_items(briefing)
        .into_iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| format!("- {text}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !needs.is_empty() {
        sections.insert("needs_attention".to_owned(), needs);
    }
    sections
}

pub fn briefing_needs_items(briefing: &Value) -> Vec<Value> {
    briefing
        .get("needs_attention")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value.is_object())
        .cloned()
        .collect()
}
pub fn briefing_meeting_count(briefing: &Value) -> usize {
    briefing
        .get("your_day")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            item.get("time")
                .and_then(Value::as_str)
                .is_some_and(|time| !time.trim().is_empty())
        })
        .count()
}

/// Report briefing existence, validity, and optional generated label.
pub fn briefing_freshness(context: &HomeContext, day: &str) -> Value {
    if !morning_briefing_path(context, day).exists() {
        return json!({"exists": false, "valid": false, "generated_label": null});
    }
    let Some(briefing) = load_briefing(context, day) else {
        return json!({"exists": true, "valid": false, "generated_label": null});
    };
    let label = briefing
        .pointer("/metadata/generated")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|time| {
            time.with_timezone(&Utc)
                .format("%-I:%M%p")
                .to_string()
                .to_lowercase()
        });
    json!({"exists": true, "valid": true, "generated_label": label})
}

pub fn compute_briefing_phase(
    segment_count: i64,
    hour: u32,
    briefing_exists: bool,
) -> &'static str {
    if hour >= BRIEFING_EOD_HOUR {
        "eod"
    } else if !briefing_exists && hour < BRIEFING_MORNING_END_HOUR {
        "pending"
    } else if briefing_exists && (segment_count == 0 || hour < BRIEFING_MORNING_END_HOUR) {
        "morning"
    } else if briefing_exists && segment_count > 0 {
        "active"
    } else {
        "eod"
    }
}

pub fn briefing_lateness_state(now: DateTime<Utc>, phase: &str) -> Value {
    let due = now
        .with_hour(BRIEFING_MORNING_END_HOUR)
        .and_then(|time| time.with_minute(0))
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0))
        .expect("valid briefing due time");
    let late = phase == "pending"
        && now.hour() > BRIEFING_MORNING_END_HOUR + BRIEFING_LATENESS_THRESHOLD_HOURS;
    json!({"late": late, "late_hours": if late { ((now - due).num_seconds() / 3600).max(0) } else { 0 }})
}

/// Count successful and failed facet-newsletter attempts for one day.
pub fn newsletter_attempts_from_think_logs(context: &HomeContext, day: &str) -> (usize, usize) {
    let successful = fs::read_dir(context.journal_root().join("facets"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .join("news")
                .join(format!("{day}.md"))
                .is_file()
        })
        .count();
    let failed = think_oplogs(context, day)
        .into_iter()
        .filter(|(run, _)| run == "daily")
        .flat_map(|(_, rows)| rows)
        .filter(|record| {
            record.get("event").and_then(Value::as_str) == Some("talent.fail")
                && record.get("facet").is_some_and(|value| match value {
                    Value::Bool(value) => *value,
                    Value::Null => false,
                    Value::Number(value) => value.as_i64() != Some(0),
                    Value::String(value) => !value.is_empty(),
                    Value::Array(value) => !value.is_empty(),
                    Value::Object(value) => !value.is_empty(),
                })
                && record.get("name").and_then(Value::as_str) == Some("facet_newsletter")
        })
        .count();
    (successful, successful + failed)
}

/// Read serialized root backlog data without generating it.
pub fn load_backlog_source(context: &HomeContext) -> BacklogSource {
    let path = context.journal_root().join("stats.json");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BacklogSource {
                backlog: None,
                validity: BacklogValidity::Missing,
                generated_at: None,
            };
        }
        Err(_) => {
            return BacklogSource {
                backlog: None,
                validity: BacklogValidity::Unparseable,
                generated_at: None,
            };
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return BacklogSource {
                backlog: None,
                validity: BacklogValidity::Unparseable,
                generated_at: None,
            };
        }
    };
    let Some(object) = value.as_object() else {
        return BacklogSource {
            backlog: None,
            validity: BacklogValidity::Malformed,
            generated_at: None,
        };
    };
    let generated_at = object
        .get("generated_at")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match object.get("backlog") {
        None => BacklogSource {
            backlog: None,
            validity: BacklogValidity::NoBacklogKey,
            generated_at,
        },
        Some(Value::Object(backlog)) => BacklogSource {
            backlog: Some(backlog.clone()),
            validity: BacklogValidity::Valid,
            generated_at,
        },
        _ => BacklogSource {
            backlog: None,
            validity: BacklogValidity::Malformed,
            generated_at,
        },
    }
}

/// Produce health-web-compatible rows from serialized backlog day entries.
pub fn stuck_day_rows(backlog: &Map<String, Value>) -> Vec<Value> {
    backlog
        .get("days")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("state").and_then(Value::as_str) == Some("stuck"))
        .cloned()
        .collect()
}

/// Read current awareness without creating the awareness directory.
pub fn load_awareness(context: &HomeContext) -> Value {
    load_current(context.journal_root()).unwrap_or_else(|_| json!({}))
}

/// Read the warning projection from `identity/health.md`.
pub fn read_steward_health(context: &HomeContext) -> Option<Value> {
    let body = fs::read_to_string(context.journal_root().join("identity/health.md")).ok()?;
    let mut section = "";
    let mut status = None;
    let mut attention = None;
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.trim();
            continue;
        }
        let text = line.trim().trim_start_matches("- ").trim();
        if text.is_empty() {
            continue;
        }
        if section == "Status" && status.is_none() {
            status = Some(text.to_owned());
        }
        if section == "Attention" && line.trim_start().starts_with("-") && attention.is_none() {
            attention = Some(text.to_owned());
        }
    }
    match (status, attention) {
        (Some(status), Some(message))
            if !status.starts_with("your journal is well.") || !message.is_empty() =>
        {
            Some(json!({"status":"warning","message":message}))
        }
        _ => None,
    }
}

/// Read and validate the latest steward accumulator summary.
pub fn read_steward_summary(context: &HomeContext, day: Option<&str>) -> Option<Value> {
    let record = read_latest(context, day.unwrap_or(&context.today()), "steward", 7)?;
    let object = record.as_object()?;
    let headline = object.get("headline")?.as_str()?.trim();
    let sentence = object.get("summary_sentence")?.as_str()?.trim();
    if headline.is_empty() || sentence.is_empty() {
        return None;
    }
    let action = object
        .get("suggested_action")
        .and_then(Value::as_str)
        .filter(|action| matches!(*action, "none" | "open_health_detail" | "open_support"))
        .unwrap_or("none");
    Some(json!({"headline":headline,"summary_sentence":sentence,"suggested_action":action}))
}

/// Scan daily health logs for unresolved agent failures and return the highest-priority
/// reference-compatible generic attention item.
pub fn resolve_attention(context: &HomeContext, awareness: &Value) -> Option<Value> {
    let day = context.today();
    let failures = think_oplogs(context, &day)
        .into_iter()
        .filter(|(run, _)| run == "daily")
        .flat_map(|(_, rows)| rows)
        .filter(|row| row.get("event").and_then(Value::as_str) == Some("talent.fail"))
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        let names = failures
            .iter()
            .filter_map(|row| row.get("name").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        let count = failures.len();
        return Some(
            json!({"placeholder_text":format!("{count} agent error{} today — ask what happened", if count == 1 { "" } else { "s" }),"context_lines":[format!("System health: {count} unresolved agent error(s) today: {}. If user asks what needs attention, summarize which agents failed.", names.into_iter().take(3).collect::<Vec<_>>().join(", "))]}),
        );
    }
    let imports = awareness.get("imports")?.as_object()?;
    let (Some(completed), Some(summary)) = (
        imports.get("last_completed").and_then(Value::as_str),
        imports.get("last_result_summary").and_then(Value::as_str),
    ) else {
        return None;
    };
    let completed = DateTime::parse_from_rfc3339(completed)
        .ok()?
        .with_timezone(&Utc);
    (context.now_utc - completed < Duration::hours(1)).then(|| json!({"placeholder_text":format!("import complete: {summary}. ask me about it"),"context_lines":[format!("System health: import recently completed — {summary}. If user asks what needs attention, mention the new import.")]}))
}

/// Parse and resolve de-duplicated `sol://` source references from text.
pub fn parse_sol_sources(text: &str) -> Vec<Value> {
    let mut sources = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in text
        .split_whitespace()
        .filter_map(|word| word.find("sol://").map(|offset| &word[offset..]))
    {
        let reference = raw.trim_end_matches(|character: char| ".,;:!?)".contains(character));
        if !seen.insert(reference.to_owned()) {
            continue;
        }
        let parts = reference
            .trim_start_matches("sol://")
            .split('/')
            .collect::<Vec<_>>();
        let is_day =
            |value: &str| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit());
        let url = if parts.len() == 4 && parts[0] == "facets" && is_day(parts[3]) {
            match parts[2] {
                "news" => Some(format!("/app/news/{}/{}", parts[1], parts[3])),
                // The reflections app is gone. Keep the citation; do not invent a URL.
                "reflections" => None,
                _ => None,
            }
        } else if parts.first().is_some_and(|part| is_day(part)) {
            Some(format!("/app/timeline/{}", parts[0]))
        } else {
            None
        };
        let label = if parts.first().is_some_and(|part| is_day(part)) {
            format!("{} {}", &parts[0][4..6], &parts[0][6..8])
        } else if parts.len() == 4 {
            format!("{} · {}", parts[1], parts[2])
        } else {
            reference.to_owned()
        };
        sources.push(json!({"ref":reference,"label":label,"url":url}));
    }
    sources
}

/// Summarize one day of health JSONL without freshness gating. The terminal-state
/// fold is delegated to system-health so outstanding failures remain distinct.
pub fn summarize_pipeline_day(context: &HomeContext, day: &str) -> Value {
    let mut summary = json!({"day":day,"generated_at":context.now_ms(),"status":"healthy","anomalies":[],"runs":{"daily":{"count":0,"duration_ms_total":0},"activity":{"count":0,"duration_ms_total":0},"on_demand":{"count":0,"duration_ms_total":0}},"talents":{"dispatched":0,"completed":0,"failed":0,"outstanding_failed":0,"skipped":0,"capped":0,"failed_list":[],"failed_list_truncated":false},"activities":{"detected":0,"persisted":0,"talents_fired":false},"exhausted_segments":{"count":0,"segments":[]}});
    let directory = day_root(context, day).join("health");
    if !directory.is_dir() {
        if day < context.today().as_str() {
            summary["status"] = "stale".into();
            summary["anomalies"]
                .as_array_mut()
                .unwrap()
                .push(json!({"kind":"segments_not_thought","error":"no_health_dir"}));
        }
        return summary;
    }
    for (run, rows) in think_oplogs(context, day) {
        let mode = match run.as_str() {
            "daily" => Some("daily"),
            "activity" => Some("activity"),
            "segment" | "segments" => Some("on_demand"),
            _ => None,
        };
        let Some(mode) = mode else {
            continue;
        };
        summary["runs"][mode]["count"] = summary["runs"][mode]["count"]
            .as_i64()
            .unwrap_or(0)
            .saturating_add(1)
            .into();
        for row in rows {
            if row
                .get("day")
                .and_then(Value::as_str)
                .is_some_and(|row_day| row_day != day)
            {
                continue;
            }
            match row.get("event").and_then(Value::as_str) {
                Some("talent.dispatch") => bump(&mut summary, "/talents/dispatched"),
                Some("talent.complete") => bump(&mut summary, "/talents/completed"),
                Some("talent.fail") => bump(&mut summary, "/talents/failed"),
                Some("talent.skip")
                    if row.get("reason").and_then(Value::as_str) == Some("capped") =>
                {
                    bump(&mut summary, "/talents/capped")
                }
                Some("talent.skip") => bump(&mut summary, "/talents/skipped"),
                Some("activity.detected") => bump(&mut summary, "/activities/detected"),
                Some("activity.persisted") => bump(&mut summary, "/activities/persisted"),
                Some("run.complete") => {
                    let duration = row.get("duration_ms").and_then(Value::as_i64).unwrap_or(0);
                    summary["runs"][mode]["duration_ms_total"] =
                        summary["runs"][mode]["duration_ms_total"]
                            .as_i64()
                            .unwrap_or(0)
                            .saturating_add(duration)
                            .into();
                }
                _ => {}
            }
            if row.get("mode").and_then(Value::as_str) == Some("activity")
                && matches!(
                    row.get("event").and_then(Value::as_str),
                    Some("talent.dispatch") | Some("talent.complete") | Some("talent.fail")
                )
            {
                summary["activities"]["talents_fired"] = true.into();
            }
        }
    }
    if summary["activities"]["detected"].as_i64().unwrap_or(0) > 0
        && !summary["activities"]["talents_fired"]
            .as_bool()
            .unwrap_or(false)
    {
        summary["anomalies"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind":"activity_agents_missing"}));
    }
    if day < context.today().as_str()
        && summary["runs"]["daily"]["count"].as_i64().unwrap_or(0) == 0
    {
        summary["anomalies"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind":"daily_agents_missing"}));
    }
    let source = FilesystemHealthLogSource::new(context.journal_root());
    if let Ok(states) = read_terminal_states(&source, day, true) {
        let mut outstanding = states
            .value
            .into_iter()
            .filter(|(_, state)| state.latest_event == TerminalEvent::Fail)
            .map(|(unit, state)| {
                json!({"mode":unit.mode,"name":unit.name,"use_id":state.use_id,"state":state.state})
            })
            .collect::<Vec<_>>();
        outstanding.sort_by(|left, right| {
            left["name"]
                .as_str()
                .cmp(&right["name"].as_str())
                .then_with(|| left["mode"].as_str().cmp(&right["mode"].as_str()))
                .then_with(|| left["use_id"].as_str().cmp(&right["use_id"].as_str()))
        });
        summary["talents"]["outstanding_failed"] = outstanding.len().into();
        summary["talents"]["failed_list"] = outstanding
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        summary["talents"]["failed_list_truncated"] = (outstanding.len() > 20).into();
        for failure in outstanding.into_iter().take(20) {
            let mut anomaly = failure.as_object().cloned().unwrap_or_default();
            anomaly.insert("kind".to_owned(), "talent_failure".into());
            summary["anomalies"]
                .as_array_mut()
                .unwrap()
                .push(Value::Object(anomaly));
        }
    }
    if summary["anomalies"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        summary["status"] = "stale".into();
    }
    summary
}

/// Resolve the current capture-health rollup from certificate-authorized clients.
pub fn get_capture_health(context: &HomeContext) -> Value {
    capture_health_json(&inspect_clients_at(
        context.journal_root(),
        context.now_ms(),
    ))
}

pub(crate) fn capture_health_json(inspection: &ClientInspection) -> Value {
    let (rows, activity, registry) = match inspection {
        ClientInspection::LedgerUnavailable { .. } => {
            return json!({
                "status": "unknown",
                "clients": [],
                "unassessed": [],
                "registry": "registry_unknown",
            });
        }
        ClientInspection::Empty { clients, activity } => {
            (clients.as_slice(), *activity, "registry_empty")
        }
        ClientInspection::Ready { clients, activity } => {
            (clients.as_slice(), *activity, "registry_complete")
        }
    };
    if matches!(
        activity,
        ClientActivityState::Unreadable | ClientActivityState::Malformed
    ) {
        return json!({
            "status": "unknown",
            "clients": [],
            "unassessed": rows.iter().map(unassessed_client_row).collect::<Vec<_>>(),
            "registry": registry,
        });
    }
    let unassessed = rows
        .iter()
        .filter(|row| row.capture_state == ClientCaptureState::NoCapture)
        .map(unassessed_client_row)
        .collect::<Vec<_>>();
    let Some(status) = rollup_client_capture_states(rows) else {
        return json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": unassessed,
            "registry": registry,
        });
    };
    let clients: Vec<Value> = rows
        .iter()
        .filter(|row| is_assessed_capture(row))
        .map(home_client_row)
        .collect();
    json!({
        "status": capture_state_name(status),
        "clients": clients,
        "unassessed": unassessed,
        "registry": registry,
    })
}

fn home_client_row(row: &ClientAssessment) -> Value {
    let mut summary = json!({
        "name": client_name(row),
        "cid": row.cid,
        "last_seen": row.last_seen_at,
        "last_accepted_ingest_at": row.last_accepted_ingest_at,
        "last_accepted_segment": row.last_accepted_segment,
        "status": capture_state_name(row.capture_state),
        "reach": reach_name(row),
    });
    if let Some(rejection) = &row.ingest_rejection {
        summary["ingest_rejection"] =
            serde_json::to_value(rejection).expect("rejection serializes");
    }
    if !row.source_delivery.is_empty() {
        summary["source_delivery"] = Value::Object(
            row.source_delivery
                .iter()
                .map(|(source, delivery)| {
                    (
                        source.clone(),
                        json!({
                            "state": source_delivery_name(delivery.state),
                            "elapsed_ms": delivery.elapsed_ms,
                            "ingest_rejection": delivery.ingest_rejection,
                        }),
                    )
                })
                .collect(),
        );
    }
    summary
}

fn source_delivery_name(state: SourceDelivery) -> &'static str {
    match state {
        SourceDelivery::Current => "current",
        SourceDelivery::NeedsAttention => "needs_attention",
        SourceDelivery::Unknown => "unknown",
    }
}

fn unassessed_client_row(row: &ClientAssessment) -> Value {
    json!({
        "name": client_name(row),
        "cid": row.cid,
        "reason": match row.capture_state {
            ClientCaptureState::NoCapture => "awaiting_first_delivery",
            ClientCaptureState::Unknown => "activity_unavailable",
            ClientCaptureState::Degraded
            | ClientCaptureState::Active
            | ClientCaptureState::Stale
            | ClientCaptureState::Offline => unreachable!("assessed capture state"),
        },
        "reach": reach_name(row),
    })
}

fn is_assessed_capture(row: &ClientAssessment) -> bool {
    matches!(
        row.capture_state,
        ClientCaptureState::Degraded
            | ClientCaptureState::Active
            | ClientCaptureState::Stale
            | ClientCaptureState::Offline
    )
}

fn client_name(row: &ClientAssessment) -> String {
    let label = row.client_entry.display_label();
    if label.is_empty() {
        row.cid.clone()
    } else {
        label
    }
}

fn capture_state_name(state: ClientCaptureState) -> &'static str {
    match state {
        ClientCaptureState::Unknown => "unknown",
        ClientCaptureState::NoCapture => "no_capture",
        ClientCaptureState::Degraded => "degraded",
        ClientCaptureState::Active => "active",
        ClientCaptureState::Stale => "stale",
        ClientCaptureState::Offline => "offline",
    }
}

fn reach_name(row: &ClientAssessment) -> &'static str {
    match row.connection {
        ConnectionFreshness::Unknown => "unknown",
        ConnectionFreshness::Known { reach, .. } => match reach {
            solstone_core_sol_link::client_status::ClientReach::Active => "active",
            solstone_core_sol_link::client_status::ClientReach::Stale => "stale",
            solstone_core_sol_link::client_status::ClientReach::Offline => "offline",
        },
    }
}

/// Newest accepted client ingest timestamp across paired clients.
pub fn last_observe_relative_seconds(context: &HomeContext) -> Option<i64> {
    let rows = match inspect_clients_at(context.journal_root(), context.now_ms()) {
        ClientInspection::Empty { clients, .. } | ClientInspection::Ready { clients, .. } => {
            clients
        }
        ClientInspection::LedgerUnavailable { .. } => return None,
    };
    rows.into_iter()
        .filter_map(|record| record.last_accepted_ingest_at)
        .filter_map(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
        .map(|timestamp| timestamp.timestamp_millis())
        .max()
        .map(|last_seen| (context.now_ms() - last_seen) / 1000)
}

/// Read the edge index for an already-resolved principal. Card projection is deliberately phase two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionReadError;

pub fn load_connections_network(
    context: &HomeContext,
    principal: &Value,
) -> Result<Option<solstone_core_indexer_query::NetworkResponse>, ConnectionReadError> {
    let Some(principal_id) = principal
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let request = NetworkRequest {
        limit: 12,
        evidence_limit: 1,
        ..NetworkRequest::default()
    };
    load_entity_network(
        context.journal_root(),
        principal_id,
        &request,
        None,
        &ATTENDANCE_KINDS,
    )
    .map(Some)
    .map_err(|_| ConnectionReadError)
}

/// Return owner-visible connection copy from its owning crate without duplication.
pub fn connection_copy() -> Value {
    ENTITIES_COPY.clone()
}

/// Build the injected-clock brain snapshot, using the health-web fallback contract on config failure.
pub fn build_brain_snapshot(context: &HomeContext) -> Value {
    let Ok(config) = solstone_core_thinking::read_config(context.journal_root()) else {
        return brain_fallback();
    };
    let inspection = inspect_brain_state(context.journal_root(), &config, context.now_utc);
    let view = present_brain_inspection(&inspection, context.now_utc);
    let projection = &inspection.projection;
    json!({"state":projection.aggregate_state,"headline":view.headline,"reason_code":projection.reason_code,"reason_text":view.reason_text,"failing_component":view.failing_component,"action":brain_action(&projection.aggregate_state, projection.reason_code.as_deref()),"identity":{"lane":projection.active_lane,"provider":projection.active_provider,"model":projection.active_model},"evidence":{"observed_at":view.evidence.observed_at,"age_seconds":view.evidence.age_seconds,"age_text":view.evidence.age_text},"components":{"generate":brain_component(inspection.record.as_ref(), "generate"),"cogitate":brain_component(inspection.record.as_ref(), "cogitate")},"progressing":projection.reason_code.as_deref() == Some("brain_check_in_progress")})
}

fn day_root(context: &HomeContext, day: &str) -> std::path::PathBuf {
    context.journal_root().join("chronicle").join(day)
}

/// Read validated structured think diagnostics from the canonical oplog namespace.
///
/// Home is an optional projection, so an unavailable journal or malformed payload is
/// treated as absent input just like the former direct JSONL reads.
fn think_oplogs(context: &HomeContext, day: &str) -> Vec<(String, Vec<Map<String, Value>>)> {
    let Ok(day_key) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return Vec::new();
    };
    let Ok(root) = JournalRoot::open(context.journal_root()) else {
        return Vec::new();
    };
    let Ok(snapshot) = catalog_oplogs(root, &[day_key]) else {
        return Vec::new();
    };
    snapshot
        .into_catalogued_entries()
        .into_iter()
        .filter_map(|(entry, mut file)| {
            let name = entry.name();
            if name.source().display_slug() != "think" || name.format() != OplogFormat::Jsonl {
                return None;
            }
            file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                .ok()?;
            let mut text = String::new();
            file.read_to_string(&mut text).ok()?;
            let rows = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|value| value.as_object().cloned())
                .collect();
            Some((name.run().display_slug().to_owned(), rows))
        })
        .collect()
}

fn read_json_value(path: &std::path::Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn read_jsonl_objects(path: &std::path::Path) -> Vec<Map<String, Value>> {
    fs::read_to_string(path)
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|value| value.as_object().cloned())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn empty_pulse() -> PulseNarrative {
    PulseNarrative {
        content: None,
        updated_at: None,
        needs: Vec::new(),
    }
}
fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn bump(summary: &mut Value, pointer: &str) {
    if let Some(value) = summary.pointer_mut(pointer) {
        *value = value.as_i64().unwrap_or(0).saturating_add(1).into();
    }
}
fn brain_fallback() -> Value {
    json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null},"cogitate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
}
fn brain_component(record: Option<&Value>, name: &str) -> Value {
    let value = record.and_then(|record| record.pointer(&format!("/evidence/{name}")));
    let reason = value
        .and_then(|item| item.get("reason_code"))
        .and_then(Value::as_str);
    json!({"status":value.and_then(|item| item.get("status")).cloned().unwrap_or(Value::Null),"reason_code":reason,"reason_text":reason.map(|text| text.replace('_', " ")).unwrap_or_else(|| "unknown".to_owned()),"observed_at":value.and_then(|item| item.get("observed_at")).cloned().unwrap_or(Value::Null)})
}
fn brain_action(state: &str, reason: Option<&str>) -> Value {
    if state == "unknown" {
        json!({"label":"check again","refresh":true})
    } else if matches!(state, "blocked" | "unhealthy")
        || (state == "unknown" && reason == Some("configuration_invalid"))
    {
        json!({"label":"open thinking","href":"/app/thinking/#main"})
    } else {
        Value::Null
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };
    use solstone_core_sol_link::client_status::{
        ClientReach, ConnectionGroup, ConnectionState, SourceDelivery, SourceDeliveryRow,
    };
    use solstone_core_sol_link::ledger::ClientEntry;
    use tempfile::TempDir;

    fn context(root: &std::path::Path) -> HomeContext {
        HomeContext::new(root, Utc.with_ymd_and_hms(2026, 6, 2, 13, 0, 0).unwrap())
    }
    fn write(root: &std::path::Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn write_oplog(root: &std::path::Path, day: &str, source: &str, run: &str, text: &str) {
        let day = NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap();
        let mut writer = create_oplog_at(
            JournalRoot::open(root).unwrap(),
            source,
            run,
            OplogFormat::Jsonl,
            opened,
        )
        .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn stats_reader_uses_raw_chronicle_day_file() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(
            root.path(),
            "chronicle/20260602/stats.json",
            r#"{"stats":{"transcript_segments":7},"facet_data":{"right":1}}"#,
        );
        write(
            root.path(),
            "20260602/stats.json",
            r#"{"stats":{"transcript_segments":8},"facet_data":{"wrong":1}}"#,
        );
        write(
            root.path(),
            "stats.json",
            r#"{"stats":{"transcript_segments":9},"facet_data":{"root":1}}"#,
        );
        assert_eq!(
            load_stats(&context, "20260602")["stats"]["transcript_segments"],
            7
        );
        assert_eq!(load_stats(&context, "20260602")["facet_data"]["right"], 1);
    }

    #[test]
    fn stats_reader_does_not_apply_freshness_gate() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(
            root.path(),
            "chronicle/20260602/stats.json",
            r#"{"stats":{"transcript_segments":3}}"#,
        );
        assert_eq!(
            load_stats(&context, "20260602")["stats"]["transcript_segments"],
            3
        );
    }

    #[test]
    fn newsletter_reader_skips_malformed_and_missing_facet_failures() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write_oplog(
            root.path(),
            "20260601",
            "think",
            "daily",
            "not json\n{\"event\":\"talent.fail\",\"name\":\"facet_newsletter\"}\n",
        );
        assert_eq!(
            newsletter_attempts_from_think_logs(&context, "20260601"),
            (0, 0)
        );
    }

    #[test]
    fn lateness_uses_supplied_phase() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 13, 17, 0).unwrap();
        assert_eq!(
            briefing_lateness_state(now, "pending"),
            json!({"late":true,"late_hours":3})
        );
    }

    #[test]
    fn reflection_has_no_dead_url() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "reflections/weekly/20260601.md", "x");
        let value = load_latest_weekly_reflection(&context).unwrap();
        assert!(value.get("url").is_none());
    }

    #[test]
    fn awareness_read_does_not_create_its_directory() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(load_awareness(&context), json!({}));
        assert!(!root.path().join("awareness").exists());
    }

    #[test]
    fn backlog_reader_distinguishes_missing_malformed_and_valid_root_documents() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::Missing
        );
        write(root.path(), "stats.json", "not json");
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::Unparseable
        );
        write(
            root.path(),
            "stats.json",
            r#"{"generated_at":"x","backlog":{"days":[]}}"#,
        );
        let source = load_backlog_source(&context);
        assert_eq!(source.validity, BacklogValidity::Valid);
        assert_eq!(source.generated_at.as_deref(), Some("x"));
    }

    #[test]
    fn day_accumulator_and_briefing_readers_handle_absent_and_malformed_inputs() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert!(read_latest(&context, "20260602", "pulse", 0).is_none());
        assert!(load_briefing(&context, "20260602").is_none());
        write(
            root.path(),
            "chronicle/20260602/talents/pulse.jsonl",
            "not json\n{\"ts\":1,\"full_details\":\"ready\"}\n",
        );
        assert_eq!(
            load_pulse_narrative(&context, "20260602")
                .content
                .as_deref(),
            Some("ready")
        );
        write(
            root.path(),
            "chronicle/20260602/talents/morning_briefing.json",
            "[]",
        );
        assert!(load_briefing(&context, "20260602").is_none());
    }

    #[test]
    fn journal_and_flow_readers_cover_missing_malformed_and_calendar_boundaries() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(count_journal_age_days(&context), 0);
        write(root.path(), "chronicle/notaday/talents/flow.md", "ignored");
        write(root.path(), "chronicle/20260530/talents/flow.md", "flow");
        assert_eq!(count_journal_age_days(&context), 3);
        assert_eq!(
            load_flow_md(&context, "20260530").content.as_deref(),
            Some("flow")
        );
        assert_eq!(load_flow_md(&context, "20260531").content, None);
        assert!(load_latest_weekly_reflection(&context).is_none());
        write(root.path(), "reflections/weekly/notaday.md", "bad");
        write(root.path(), "reflections/weekly/20260531.md", "good");
        assert_eq!(
            load_latest_weekly_reflection(&context).unwrap()["day"],
            "20260531"
        );
        write(
            root.path(),
            "reflections/weekly/99999999.md",
            "future-looking",
        );
        assert_eq!(
            load_latest_weekly_reflection(&context).unwrap(),
            json!({"day":"99999999","label":"99999999"})
        );
    }

    #[test]
    fn stats_readers_treat_missing_and_malformed_as_no_data() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(load_stats(&context, "20260602"), json!({}));
        write(root.path(), "chronicle/20260602/stats.json", "{");
        assert_eq!(load_stats(&context, "20260602"), json!({}));
        assert_eq!(load_yesterday_stats(&context), None);
        write(
            root.path(),
            "chronicle/20260601/stats.json",
            r#"{"stats":{"transcript_segments":2}}"#,
        );
        assert_eq!(
            load_yesterday_stats(&context).unwrap()["stats"]["transcript_segments"],
            2
        );
    }

    #[test]
    fn facet_readers_include_muted_only_where_the_reference_requires() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(
            root.path(),
            "facets/visible/facet.json",
            r#"{"muted":false}"#,
        );
        write(root.path(), "facets/muted/facet.json", r#"{"muted":true}"#);
        write(
            root.path(),
            "facets/undeclared/activities/20260602.jsonl",
            r#"{"source":"anticipated","title":"ignored"}"#,
        );
        write(
            root.path(),
            "facets/visible/activities/20260602.jsonl",
            r#"{"source":"anticipated","title":"visible","start":"10:00","end":"11:00"}"#,
        );
        write(
            root.path(),
            "facets/muted/activities/20260602.jsonl",
            r#"{"source":"anticipated","title":"muted","start":"10:00","end":"11:00"}"#,
        );
        assert_eq!(all_facet_names(&context), vec!["muted", "visible"]);
        assert_eq!(enabled_facet_names(&context), vec!["visible"]);
        let titles = collect_anticipated_activities(&context, "20260602")
            .into_iter()
            .map(|row| row["title"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(titles, vec!["muted", "visible"]);
        assert!(collect_top_activities_yesterday(&context).is_empty());
    }

    #[test]
    fn facet_readers_ignore_malformed_declarations_and_activity_rows() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "facets/array/facet.json", "[]");
        write(root.path(), "facets/broken/facet.json", "{");
        write(root.path(), "facets/declared/facet.json", "{}");
        write(
            root.path(),
            "facets/declared/activities/20260602.jsonl",
            "not json\n[]\n",
        );
        assert_eq!(all_facet_names(&context), vec!["declared"]);
        assert!(enabled_facet_names(&context).contains(&"declared".to_owned()));
        assert!(collect_anticipated_activities(&context, "20260602").is_empty());
        assert!(collect_activities(&context, "20260602").is_empty());
    }

    #[test]
    fn activity_reader_includes_muted_declared_facets_but_not_undeclared_ones() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        let recent = context.now_ms() - 1;
        write(root.path(), "facets/muted/facet.json", r#"{"muted":true}"#);
        write(
            root.path(),
            "facets/muted/activities/20260602.jsonl",
            &format!(r#"{{"source":"user","created_at":{recent},"title":"muted"}}"#),
        );
        write(
            root.path(),
            "facets/undeclared/activities/20260602.jsonl",
            &format!(r#"{{"source":"user","created_at":{recent},"title":"ignored"}}"#),
        );
        let rows = collect_activities(&context, "20260602");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["facet"], "muted");
    }

    #[test]
    fn activity_reader_repairs_invalid_created_timestamp_and_honors_cutoff() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "facets/work/facet.json", "{}");
        write(
            root.path(),
            "facets/work/activities/20260602.jsonl",
            r#"{"source":"user","created_at":1780405200000,"title":"recent"}
{"source":"user","created_at":999999999999999999,"title":"invalid"}
{"source":"user","created_at":1780387199999,"title":"old"}"#,
        );
        let rows = collect_activities(&context, "20260602");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["title"], "invalid");
        assert_eq!(rows[0]["display_time"], "");
        assert_eq!(rows[1]["title"], "recent");
    }

    #[test]
    fn briefing_readers_cover_required_shape_and_guard_repairs() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(
            morning_briefing_path(&context, "20260602"),
            root.path()
                .join("chronicle/20260602/talents/morning_briefing.json")
        );
        assert_eq!(
            briefing_freshness(&context, "20260602"),
            json!({"exists":false,"valid":false,"generated_label":null})
        );
        write(
            root.path(),
            "chronicle/20260602/talents/morning_briefing.json",
            r#"{"metadata":{"generated":"invalid"},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        );
        assert_eq!(
            load_briefing(&context, "20260602").unwrap()["metadata"]["generated"],
            "invalid"
        );
        assert_eq!(
            briefing_freshness(&context, "20260602"),
            json!({"exists":true,"valid":true,"generated_label":null})
        );
        assert_eq!(
            briefing_meeting_count(&json!({"your_day":[{"time":""},{"time":"10:00"}]})),
            1
        );
        assert_eq!(
            briefing_needs_items(&json!({"needs_attention":[{},"bad"]})),
            vec![json!({})]
        );
        assert_eq!(
            render_briefing_sections(
                &json!({"yesterday":["done"],"needs_attention":[{"text":"act"}]})
            )["yesterday"],
            "- done"
        );
    }

    #[test]
    fn briefing_phase_and_lateness_cover_both_guard_directions() {
        let now = Utc.with_ymd_and_hms(2026, 6, 2, 13, 0, 0).unwrap();
        assert_eq!(compute_briefing_phase(0, 9, false), "pending");
        assert_eq!(compute_briefing_phase(1, 13, true), "active");
        assert_eq!(
            briefing_lateness_state(now, "pending"),
            json!({"late":true,"late_hours":3})
        );
        assert_eq!(
            briefing_lateness_state(now, "active"),
            json!({"late":false,"late_hours":0})
        );
        assert_eq!(
            briefing_lateness_state(
                Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
                "pending"
            ),
            json!({"late":false,"late_hours":0})
        );
    }

    #[test]
    fn backlog_and_steward_readers_distinguish_missing_malformed_and_value_cases() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert!(stuck_day_rows(&Map::new()).is_empty());
        assert_eq!(
            stuck_day_rows(
                &serde_json::from_str(r#"{"days":[{"state":"stuck"},{"state":"ready"}]}"#).unwrap()
            ),
            vec![json!({"state":"stuck"})]
        );
        assert_eq!(read_steward_health(&context), None);
        write(
            root.path(),
            "identity/health.md",
            "## Status\n- degraded\n## Attention\n- repair input\n",
        );
        assert_eq!(
            read_steward_health(&context).unwrap(),
            json!({"status":"warning","message":"repair input"})
        );
        assert_eq!(read_steward_summary(&context, Some("20260602")), None);
        write(
            root.path(),
            "chronicle/20260602/talents/steward.jsonl",
            r#"{"ts":1,"headline":"h","summary_sentence":"s","suggested_action":"bad"}"#,
        );
        assert_eq!(
            read_steward_summary(&context, Some("20260602")).unwrap(),
            json!({"headline":"h","summary_sentence":"s","suggested_action":"none"})
        );
    }

    #[test]
    fn steward_and_attention_readers_reject_malformed_records_and_use_recent_imports() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "identity/health.md", "not a steward document");
        assert_eq!(read_steward_health(&context), None);
        write(
            root.path(),
            "chronicle/20260602/talents/steward.jsonl",
            r#"{"ts":1,"headline":"","summary_sentence":"s"}"#,
        );
        assert_eq!(read_steward_summary(&context, Some("20260602")), None);
        assert_eq!(
            resolve_attention(
                &context,
                &json!({"imports":{"last_completed":"not-a-timestamp","last_result_summary":"done"}})
            ),
            None
        );
        let awareness = json!({"imports":{"last_completed":"2026-06-02T12:30:00Z","last_result_summary":"done"}});
        assert_eq!(
            resolve_attention(&context, &awareness).unwrap()["placeholder_text"],
            "import complete: done. ask me about it"
        );
    }

    #[test]
    fn attention_sources_and_pipeline_summary_skip_malformed_rows() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(resolve_attention(&context, &json!({})), None);
        write_oplog(
            root.path(),
            "20260602",
            "think",
            "daily",
            "bad\n{\"event\":\"talent.fail\",\"name\":\"writer\"}\n",
        );
        assert!(
            resolve_attention(&context, &json!({})).unwrap()["placeholder_text"]
                .as_str()
                .unwrap()
                .contains("1 agent error")
        );
        let summary = summarize_pipeline_day(&context, "20260602");
        assert_eq!(summary["talents"]["failed"], 1);
        let missing = summarize_pipeline_day(&context, "20260601");
        assert_eq!(missing["status"], "stale");
    }

    #[test]
    fn source_parser_and_awareness_reader_cover_absent_malformed_and_value_cases() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(parse_sol_sources("no sources"), Vec::<Value>::new());
        assert_eq!(
            parse_sol_sources("sol://20260602/a sol://20260602/a").len(),
            1
        );
        assert_eq!(load_awareness(&context), json!({}));
        write(root.path(), "awareness/current.json", "bad");
        assert_eq!(load_awareness(&context), json!({}));
        write(
            root.path(),
            "awareness/current.json",
            r#"{"imports":{"last_result_summary":"ok"}}"#,
        );
        assert_eq!(
            load_awareness(&context)["imports"]["last_result_summary"],
            "ok"
        );
    }

    fn assessment(
        cid: &str,
        capture_state: ClientCaptureState,
        capture_elapsed_ms: Option<i64>,
        reach: ClientReach,
    ) -> ClientAssessment {
        ClientAssessment {
            cid: cid.to_owned(),
            client_entry: ClientEntry::new(
                cid,
                cid,
                "2026-01-01T00:00:00Z",
                "instance",
                Default::default(),
            ),
            last_seen_at: None,
            last_accepted_ingest_at: None,
            last_accepted_segment: None,
            ingest_rejection: None,
            connection: ConnectionFreshness::Known {
                state: ConnectionState::Connected,
                group: ConnectionGroup::Active,
                elapsed_ms: Some(1),
                clock_skew: false,
                label: "connected",
                reach,
            },
            capture_state,
            capture_elapsed_ms,
            source_delivery: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn capture_health_projects_client_capture_states() {
        let inspection = ClientInspection::Ready {
            clients: vec![
                assessment(
                    "awaiting",
                    ClientCaptureState::NoCapture,
                    None,
                    ClientReach::Active,
                ),
                assessment(
                    "active",
                    ClientCaptureState::Active,
                    Some(1_000),
                    ClientReach::Active,
                ),
                assessment(
                    "stale",
                    ClientCaptureState::Stale,
                    Some(120_000),
                    ClientReach::Stale,
                ),
            ],
            activity: ClientActivityState::Present,
        };
        let health = capture_health_json(&inspection);
        assert_eq!(health["status"], "stale");
        assert_eq!(health["registry"], "registry_complete");
        assert_eq!(health["clients"].as_array().unwrap().len(), 2);
        assert_eq!(health["unassessed"][0]["name"], "awaiting");
        assert_eq!(health["unassessed"][0]["reason"], "awaiting_first_delivery");

        let degraded = ClientInspection::Ready {
            clients: vec![assessment(
                "failing",
                ClientCaptureState::Degraded,
                None,
                ClientReach::Active,
            )],
            activity: ClientActivityState::Present,
        };
        assert_eq!(capture_health_json(&degraded)["status"], "degraded");
    }

    #[test]
    fn capture_health_keeps_activity_failure_distinct_from_no_capture() {
        let unknown = ClientInspection::Ready {
            clients: vec![assessment(
                "phone",
                ClientCaptureState::Unknown,
                None,
                ClientReach::Offline,
            )],
            activity: ClientActivityState::Malformed,
        };
        let health = capture_health_json(&unknown);
        assert_eq!(health["status"], "unknown");
        assert_eq!(health["registry"], "registry_complete");
        assert_eq!(health["unassessed"][0]["reason"], "activity_unavailable");

        let missing = ClientInspection::Ready {
            clients: vec![assessment(
                "phone",
                ClientCaptureState::NoCapture,
                None,
                ClientReach::Offline,
            )],
            activity: ClientActivityState::Missing,
        };
        assert_eq!(capture_health_json(&missing)["status"], "no_clients");
    }

    #[test]
    fn client_reader_uses_accepted_ingest_for_capture_health_and_last_observe() {
        let root = TempDir::new().unwrap();
        let home_context = context(root.path());
        let now = home_context.now_ms();
        let timestamp = Utc
            .timestamp_millis_opt(now - 29_000)
            .single()
            .unwrap()
            .to_rfc3339();
        write(
            root.path(),
            "link/authorized_clients.json",
            r#"[{"fingerprint":"cid","device_label":"phone","paired_at":"2026-01-01T00:00:00Z","instance_id":"instance","kind":"cert"}]"#,
        );
        write(
            root.path(),
            "link/devices.json",
            &json!({"cid": {"last_seen_at": timestamp, "last_accepted_ingest_at": timestamp}})
                .to_string(),
        );
        let health = get_capture_health(&home_context);
        assert_eq!(health["status"], "active");
        assert_eq!(health["clients"][0]["name"], "phone");
        assert!(health["clients"][0].get("source_delivery").is_none());
        assert_eq!(last_observe_relative_seconds(&home_context), Some(29));
    }

    #[test]
    fn capture_health_json_omits_source_delivery_when_empty_and_emits_it_additively() {
        let mut single = assessment(
            "phone",
            ClientCaptureState::Active,
            Some(1_000),
            ClientReach::Active,
        );
        single.source_delivery.insert(
            "audio".to_owned(),
            SourceDeliveryRow {
                state: SourceDelivery::Current,
                elapsed_ms: Some(1_000),
                last_accepted_ingest_at: Some("2026-01-01T00:00:00Z".to_owned()),
                last_accepted_segment: None,
                ingest_rejection: None,
            },
        );
        let empty = assessment(
            "phone",
            ClientCaptureState::Active,
            Some(1_000),
            ClientReach::Active,
        );
        let empty_json = capture_health_json(&ClientInspection::Ready {
            clients: vec![empty],
            activity: ClientActivityState::Present,
        });
        let single_json = capture_health_json(&ClientInspection::Ready {
            clients: vec![single],
            activity: ClientActivityState::Present,
        });
        assert!(empty_json["clients"][0].get("source_delivery").is_none());
        assert_eq!(empty_json["clients"][0]["status"], "active");
        assert_eq!(
            single_json["clients"][0]["source_delivery"]["audio"]["state"],
            "current"
        );
        assert_eq!(
            single_json["clients"][0]["source_delivery"]["audio"]["elapsed_ms"],
            1_000
        );
        assert!(
            single_json["clients"][0]["source_delivery"]["audio"]["ingest_rejection"].is_null()
        );

        let mut unnamed = assessment(
            "phone",
            ClientCaptureState::Active,
            Some(1_000),
            ClientReach::Active,
        );
        unnamed.source_delivery.insert(
            String::new(),
            SourceDeliveryRow {
                state: SourceDelivery::Current,
                elapsed_ms: Some(1_000),
                last_accepted_ingest_at: None,
                last_accepted_segment: None,
                ingest_rejection: None,
            },
        );
        let unnamed_json = capture_health_json(&ClientInspection::Ready {
            clients: vec![unnamed],
            activity: ClientActivityState::Present,
        });
        assert_eq!(
            unnamed_json["clients"][0]["source_delivery"][""]["state"],
            "current"
        );
        assert_eq!(unnamed_json["status"], empty_json["status"]);
        assert_eq!(unnamed_json["clients"][0]["status"], "active");
    }

    #[test]
    fn capture_health_json_projects_multi_source_needs_attention() {
        let mut row = assessment(
            "phone",
            ClientCaptureState::Active,
            Some(1_000),
            ClientReach::Active,
        );
        row.source_delivery.insert(
            "audio".to_owned(),
            SourceDeliveryRow {
                state: SourceDelivery::Current,
                elapsed_ms: Some(1_000),
                last_accepted_ingest_at: None,
                last_accepted_segment: None,
                ingest_rejection: None,
            },
        );
        row.source_delivery.insert(
            "location".to_owned(),
            SourceDeliveryRow {
                state: SourceDelivery::NeedsAttention,
                elapsed_ms: Some(700_000),
                last_accepted_ingest_at: None,
                last_accepted_segment: None,
                ingest_rejection: None,
            },
        );
        let health = capture_health_json(&ClientInspection::Ready {
            clients: vec![row],
            activity: ClientActivityState::Present,
        });
        assert_eq!(health["status"], "active");
        assert_eq!(
            health["clients"][0]["source_delivery"]["location"]["state"],
            "needs_attention"
        );
        assert_eq!(
            health["clients"][0]["source_delivery"]["audio"]["state"],
            "current"
        );
    }

    #[test]
    fn connections_acquisition_and_brain_fallback_have_explicit_contracts() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert!(matches!(
            load_connections_network(&context, &json!({})),
            Ok(None)
        ));
        assert!(connection_copy().is_object());
        assert!(build_brain_snapshot(&context).is_object());
        assert_eq!(
            brain_fallback(),
            json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null},"cogitate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
        );
    }

    #[test]
    fn latest_reader_uses_latest_timestamp_and_respects_lookback_boundary() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(
            root.path(),
            "chronicle/20260601/talents/pulse.jsonl",
            "bad\n{\"ts\":1,\"value\":\"older\"}\n{\"ts\":2,\"value\":\"latest\"}",
        );
        assert_eq!(read_latest(&context, "20260602", "pulse", 0), None);
        assert_eq!(
            read_latest(&context, "20260602", "pulse", 1).unwrap()["value"],
            "latest"
        );
        assert_eq!(read_latest(&context, "notaday", "pulse", 1), None);
    }

    #[test]
    fn pulse_reader_requires_nonempty_details_and_skips_malformed_rows() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(load_pulse_narrative(&context, "20260602").content, None);
        write(
            root.path(),
            "chronicle/20260602/talents/pulse.jsonl",
            "bad\n{\"ts\":1,\"full_details\":\"  \"}\n",
        );
        assert_eq!(load_pulse_narrative(&context, "20260602").content, None);
        write(
            root.path(),
            "chronicle/20260602/talents/pulse.jsonl",
            r#"{"ts":1780405200000,"full_details":"details","needs_you":["follow up",""]}"#,
        );
        let pulse = load_pulse_narrative(&context, "20260602");
        assert_eq!(pulse.content.as_deref(), Some("details"));
        assert_eq!(pulse.needs, vec!["follow up"]);
    }

    #[test]
    fn top_activity_reader_excludes_muted_facets_and_uses_native_duration() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "facets/visible/facet.json", "{}");
        write(root.path(), "facets/muted/facet.json", r#"{"muted":true}"#);
        write(
            root.path(),
            "facets/visible/activities/20260601.jsonl",
            r#"{"description":"deep work","segments":["20260601-100000-110000"]}"#,
        );
        write(
            root.path(),
            "facets/muted/activities/20260601.jsonl",
            r#"{"description":"hidden","segments":["20260601-100000-120000"]}"#,
        );
        let rows = collect_top_activities_yesterday(&context);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["title"], "deep work");
        assert_eq!(rows[0]["facet"], "visible");
    }

    #[test]
    fn briefing_load_rejects_malformed_and_incomplete_documents() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert!(load_briefing(&context, "20260602").is_none());
        write(
            root.path(),
            "chronicle/20260602/talents/morning_briefing.json",
            "bad",
        );
        assert!(load_briefing(&context, "20260602").is_none());
        write(
            root.path(),
            "chronicle/20260602/talents/morning_briefing.json",
            r#"{"metadata":{}}"#,
        );
        assert!(load_briefing(&context, "20260602").is_none());
    }

    #[test]
    fn briefing_renderer_omits_empty_sections_and_formats_all_section_kinds() {
        let rendered = render_briefing_sections(
            &json!({"yesterday":[""],"forward_look":["next"],"your_day":[{"time":"9:00","text":"meeting"}],"reading":[{"facet":"work","summary":"read"}],"needs_attention":[{"text":"act"}]}),
        );
        assert!(!rendered.contains_key("yesterday"));
        assert_eq!(rendered["forward_look"], "- next");
        assert_eq!(rendered["your_day"], "- **9:00** — meeting");
        assert_eq!(rendered["reading"], "- **work** — read");
        assert_eq!(rendered["needs_attention"], "- act");
    }

    #[test]
    fn newsletter_reader_counts_present_news_and_qualified_failures_only() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "facets/work/news/20260602.md", "sent");
        write_oplog(
            root.path(),
            "20260602",
            "think",
            "daily",
            r#"{"event":"talent.fail","name":"facet_newsletter","facet":"work"}
{"event":"talent.fail","name":"other","facet":"work"}
{"event":"talent.fail","name":"facet_newsletter","facet":false}"#,
        );
        assert_eq!(
            newsletter_attempts_from_think_logs(&context, "20260602"),
            (1, 2)
        );
    }

    #[test]
    fn backlog_reader_preserves_no_key_and_non_object_distinctions() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "stats.json", "[]");
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::Malformed
        );
        write(root.path(), "stats.json", r#"{"generated_at":"x"}"#);
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::NoBacklogKey
        );
        write(root.path(), "stats.json", r#"{"backlog":[]}"#);
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::Malformed
        );
    }

    #[test]
    fn awareness_and_connections_readers_keep_absence_nonfatal() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(load_awareness(&context), json!({}));
        assert!(matches!(
            load_connections_network(&context, &json!({})),
            Ok(None)
        ));
        assert!(matches!(
            load_connections_network(&context, &json!({"id":""})),
            Ok(None)
        ));
    }

    #[test]
    fn pipeline_reader_ignores_wrong_day_and_unknown_log_modes() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write_oplog(
            root.path(),
            "20260602",
            "heartbeat",
            "pass",
            r#"{"event":"talent.fail"}"#,
        );
        write_oplog(
            root.path(),
            "20260602",
            "think",
            "daily",
            r#"bad
{"day":"20260601","event":"talent.fail"}
{"day":"20260602","event":"talent.complete"}"#,
        );
        let summary = summarize_pipeline_day(&context, "20260602");
        assert_eq!(summary["talents"]["failed"], 0);
        assert_eq!(summary["talents"]["completed"], 1);
        assert_eq!(summary["runs"]["daily"]["count"], 1);
    }

    #[test]
    fn brain_fallback_is_the_health_web_partial_failure_contract() {
        assert_eq!(
            brain_fallback(),
            json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null},"cogitate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
        );
    }
}
