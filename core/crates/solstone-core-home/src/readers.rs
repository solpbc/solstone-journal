// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Filesystem and native-store readers. This module performs no writes.
//!
//! It remains a single reader module until the pure projections are added as
//! separate modules. Those projections will have neither filesystem imports
//! nor a journal-root input, which makes their no-I/O boundary explicit.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{
    DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc,
};
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection};
use solstone_core_entities::{ATTENDANCE_KINDS, ENTITIES_COPY};
use solstone_core_facets::{
    list_declared_facet_names, load_activity_records, load_current, read_facet_declaration,
};
use solstone_core_indexer_query::{NetworkRequest, load_entity_network};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, fold_oplogs},
};
use solstone_core_journal_stats_cli::estimate_duration_minutes;
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientCaptureState, ClientInspection,
    ConnectionFreshness, SourceDelivery, inspect_clients_at, rollup_client_capture_states,
};
use solstone_core_speaker_resolve::owner_provisional::{OwnerTierOutcome, resolve_owner_tier};
use solstone_core_system_health::{FilesystemHealthLogSource, TerminalEvent, read_terminal_states};

use crate::HomeContext;
use crate::model::{BacklogSource, BacklogValidity, FlowDocument, PulseNarrative};

const BRIEFING_MORNING_END_HOUR: u32 = solstone_core_system_health::OVERNIGHT_WINDOW_END_HOUR;
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
        .map(|day| (context.local_date() - day).num_days().max(0))
        .unwrap_or(0)
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
        .map(|time| time.with_timezone(&Utc).to_rfc3339());
    PulseNarrative {
        content: Some(content.to_owned()),
        updated_at,
        needs,
        window: record
            .get("window")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
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

/// The local clock time a segment directory name stands for. `HHMMSS_<length>`
/// is the reference spelling; anything else has no time of its own.
fn segment_clock(segment: &str) -> Option<NaiveTime> {
    let (clock, length) = segment.split_once('_')?;
    if clock.len() != 6
        || !clock.bytes().all(|byte| byte.is_ascii_digit())
        || length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    NaiveTime::parse_from_str(clock, "%H%M%S").ok()
}

/// When an activity happened: the start of the earliest segment it covers, in
/// the day's own local clock. `created_at` is when the record was written,
/// which is a different fact — a talent run that writes an evening's records at
/// 10:55 PM stamps every one of them 10:55 PM. G1-201.
fn activity_started_at(day: &str, record: &Map<String, Value>) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let start = record
        .get("segments")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(segment_clock)
        .min()?;
    Some(date.and_time(start))
}

/// An activity that spans two facets is written once per facet under one id, so
/// the pulse listed it twice and counted it twice. Collapse the copies onto the
/// first one, keeping every facet it touched and every description written for
/// it rather than dropping one. G1-205.
fn merge_faceted_activity(kept: &mut Map<String, Value>, other: &Map<String, Value>) {
    let mut facets = kept
        .get("facets")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            kept.get("facet")
                .and_then(Value::as_str)
                .map(|facet| vec![facet.to_owned()])
        })
        .unwrap_or_default();
    if let Some(facet) = other.get("facet").and_then(Value::as_str)
        && !facets.iter().any(|known| known == facet)
    {
        facets.push(facet.to_owned());
    }
    kept.insert("facets".to_owned(), facets.into());
    for field in ["description", "title"] {
        let addition = other
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let existing = kept
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if addition.is_empty() || existing.contains(addition) {
            continue;
        }
        let merged = if existing.is_empty() {
            addition.to_owned()
        } else {
            format!("{existing} {addition}")
        };
        kept.insert(field.to_owned(), merged.into());
    }
}

/// Collect non-anticipated activity records created within four hours of the injected instant.
///
/// Each row carries `display_time`: when the activity happened, not when its
/// record was written, and the list is ordered by the same value.
pub fn collect_activities(context: &HomeContext, day: &str) -> Vec<Value> {
    let cutoff = context.now_ms() - 4 * 60 * 60 * 1000;
    let zone = context.zone();
    let mut collected: Vec<Map<String, Value>> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    // The whole day's records, not only the recent window: a row's concurrent
    // partner may have been written before the window opened.
    let day_records = all_facet_names(context)
        .into_iter()
        .flat_map(|facet| {
            load_activity_records(context.journal_root(), &facet, day, true)
                .unwrap_or_default()
                .into_iter()
                .filter(|record| {
                    record.get("source").and_then(Value::as_str) != Some("anticipated")
                })
                .map(move |record| (facet.clone(), record))
        })
        .collect::<Vec<_>>();
    let source_labels = day_source_labels(context, day, &day_records);
    for ((facet, mut record), source_label) in day_records.into_iter().zip(source_labels) {
        let created = record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if created < cutoff {
            continue;
        }
        if let Some(label) = source_label {
            record.insert("source_label".to_owned(), label.into());
        }
        // One shape for both arms: RFC 3339 in the journal's day
        // coordinate. The segment arm used to emit a naive local time and
        // the write-time arm a UTC instant, so two rows on one list were
        // read on two different clocks (F-6).
        record.insert(
            "display_time".to_owned(),
            activity_started_at(day, &record)
                .and_then(|start| zone.from_local_datetime(&start).earliest())
                .or_else(|| {
                    DateTime::from_timestamp_millis(created).map(|time| time.with_timezone(&zone))
                })
                .map(|time| time.to_rfc3339())
                .unwrap_or_default()
                .into(),
        );
        record.insert("facet".to_owned(), facet.clone().into());
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match positions.get(&id) {
            Some(&position) if !id.is_empty() => {
                let (kept, merged) = (&mut collected[position], &record);
                merge_faceted_activity(kept, merged);
                // Concurrency is per facet, so the copy that ran alongside
                // another stream lends the merged row its source.
                if !kept.contains_key("source_label")
                    && let Some(label) = merged.get("source_label")
                {
                    kept.insert("source_label".to_owned(), label.clone());
                }
            }
            _ => {
                if !id.is_empty() {
                    positions.insert(id, collected.len());
                }
                collected.push(record);
            }
        }
    }
    let mut rows = collected
        .into_iter()
        .map(|record| {
            let ordered = activity_started_at(day, &record)
                .and_then(|start| zone.from_local_datetime(&start).earliest())
                .map(|start| start.timestamp_millis())
                .or_else(|| record.get("created_at").and_then(Value::as_i64))
                .unwrap_or(0);
            (ordered, Value::Object(record))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(ordered, _)| std::cmp::Reverse(*ordered));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The source phrase for each of a day's records, in order: set only on a
/// record that ran alongside a same-facet record from another capture stream.
fn day_source_labels(
    context: &HomeContext,
    day: &str,
    records: &[(String, Map<String, Value>)],
) -> Vec<Option<String>> {
    let placed = records
        .iter()
        .map(|(facet, record)| crate::sources::Placed { facet, record })
        .collect::<Vec<_>>();
    crate::sources::concurrent_source_labels(context.journal_root(), day, &placed)
}

/// Collect enabled-facet activity records and use the native duration estimator.
pub fn collect_top_activities_yesterday(context: &HomeContext) -> Vec<Value> {
    let day = context.yesterday();
    let day_records = enabled_facet_names(context)
        .into_iter()
        .flat_map(|facet| {
            load_activity_records(context.journal_root(), &facet, &day, true)
                .unwrap_or_default()
                .into_iter()
                .map(move |record| (facet.clone(), record))
        })
        .collect::<Vec<_>>();
    let source_labels = day_source_labels(context, &day, &day_records);
    let mut rows = day_records
        .into_iter()
        .zip(source_labels)
        .map(|((facet, mut record), source_label)| {
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
            record.insert("facet".to_owned(), facet.into());
            record.insert("title".to_owned(), title.into());
            record.insert(
                "duration_minutes".to_owned(),
                estimate_duration_minutes(&segments).into(),
            );
            if let Some(label) = source_label {
                record.insert("source_label".to_owned(), label.into());
            }
            Value::Object(record)
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

/// Read the daily artifact that supplies the requested presentation day.
pub fn morning_briefing_path(context: &HomeContext, day: &str) -> Option<std::path::PathBuf> {
    let presentation = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let dates = crate::briefing::BriefingDates::for_presentation(presentation)?;
    Some(
        day_root(context, &dates.analysis.format("%Y%m%d").to_string())
            .join("talents/morning_briefing.json"),
    )
}

/// Load only a briefing document with the required root keys.
pub fn load_briefing(context: &HomeContext, day: &str) -> Option<Value> {
    let briefing = read_json_value(&morning_briefing_path(context, day)?)?;
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
    {
        let (key, fields) = ("reading", ("facet", "summary"));
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
                        (false, false) => format!("- **{left}**: {right}"),
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
    let your_day = briefing
        .get("your_day")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|row| {
            let text = row.get("text").and_then(Value::as_str).unwrap_or("").trim();
            let label = your_day_time_label(row);
            (!label.is_empty() || !text.is_empty()).then(|| {
                match (label.is_empty(), text.is_empty()) {
                    (false, false) => format!("- **{label}**: {text}"),
                    (false, true) => format!("- **{label}**"),
                    (true, false) => format!("- {text}"),
                    _ => String::new(),
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !your_day.is_empty() {
        sections.insert("your_day".to_owned(), your_day);
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
/// A `your_day` item's clock label, mirroring `solstone-core-format`'s
/// `content::morning_briefing::time_label` and this crate's own
/// `briefing::your_day_time_label` (three call sites, three crates/modules;
/// duplicated rather than shared -- each renders a different surface).
/// `start == end` (or either one blank) collapses to a single point; `end`
/// alone with no `start` still reads as a point-in-time at `end`.
fn your_day_time_label(row: &serde_json::Map<String, Value>) -> String {
    let field = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let start = field("start");
    let end = field("end");
    match (start.is_empty(), end.is_empty()) {
        (true, true) => String::new(),
        (false, true) => start.to_owned(),
        (true, false) => end.to_owned(),
        (false, false) if start == end => start.to_owned(),
        (false, false) => format!("{start}–{end}"),
    }
}

pub fn briefing_meeting_count(briefing: &Value) -> usize {
    briefing
        .get("your_day")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|item| !your_day_time_label(item).is_empty())
        .count()
}

/// Report briefing existence, validity, and optional generated label.
pub fn briefing_freshness(context: &HomeContext, day: &str) -> Value {
    if !morning_briefing_path(context, day).is_some_and(|path| path.is_file()) {
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

/// Whether the overnight window — the hours in which the overnight review and
/// the morning briefing are produced — has actually passed for the local day.
/// Before it has, nothing overnight can be reported as having failed to finish.
pub fn overnight_window_passed(local_hour: u32) -> bool {
    local_hour >= BRIEFING_MORNING_END_HOUR
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
    } else if !briefing_exists {
        // Between the morning end and the evening a briefing that was never
        // prepared used to fall through to "eod", which renders no card at all.
        // Silence is the worst failure mode: name the gap instead.
        "missing"
    } else {
        "eod"
    }
}

/// The briefing clock is a wall-clock, day-scoped one: `now` is the instant in
/// the journal's local day coordinate, not UTC.
pub fn briefing_lateness_state(now: DateTime<FixedOffset>, phase: &str) -> Value {
    let due = now
        .with_hour(BRIEFING_MORNING_END_HOUR)
        .and_then(|time| time.with_minute(0))
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0))
        .expect("valid briefing due time");
    let late = phase == "missing"
        || (phase == "pending"
            && now.hour() > BRIEFING_MORNING_END_HOUR + BRIEFING_LATENESS_THRESHOLD_HOURS);
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
        .unwrap_or_default()
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
            let validity = solstone_core_system_health::summary_not_yet(
                context.journal_root(),
                context.now_local().naive_local(),
            )
            .map_or(BacklogValidity::Missing, BacklogValidity::NotYet);
            return BacklogSource {
                backlog: None,
                validity,
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

/// Scan daily health logs for unresolved agent failures and return the highest-priority
/// reference-compatible generic attention item.
pub fn resolve_attention(context: &HomeContext, awareness: &Value) -> Option<Value> {
    let day = context.today();
    let failures = think_oplogs(context, &day)
        .unwrap_or_default()
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
            json!({"placeholder_text":format!("{count} agent error{} today. ask what happened", if count == 1 { "" } else { "s" }),"context_lines":[format!("System health: {count} unresolved agent error(s) today: {}. If user asks what needs attention, summarize which agents failed.", names.into_iter().take(3).collect::<Vec<_>>().join(", "))]}),
        );
    }
    let imports = awareness.get("imports")?.as_object()?;
    let (Some(completed), Some(summary)) = (
        imports.get("last_completed").and_then(Value::as_str),
        imports.get("last_result_summary").and_then(Value::as_str),
    ) else {
        return None;
    };
    // The imports block records local wall time (`20260515T12:00:00`), read in
    // the journal's day coordinate; an RFC 3339 instant is accepted as well.
    let completed = NaiveDateTime::parse_from_str(completed, "%Y%m%dT%H:%M:%S")
        .ok()
        .and_then(|wall| {
            wall.and_local_timezone(context.zone())
                .earliest()
                .map(|local| local.with_timezone(&Utc))
        })
        .or_else(|| {
            DateTime::parse_from_rfc3339(completed)
                .ok()
                .map(|instant| instant.with_timezone(&Utc))
        })?;
    (context.now_utc - completed < Duration::hours(1)).then(|| json!({"placeholder_text":"import complete.".to_owned(),"context_lines":[format!("System health: import recently completed — {summary}. If user asks what needs attention, mention the new import.")]}))
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
    let Ok(logs) = think_oplogs(context, day) else {
        summary["status"] = "unknown".into();
        summary["anomalies"] = json!([{"kind":"pipeline_unavailable","error":"scan_failed"}]);
        return summary;
    };
    for (run, rows) in logs {
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
    } else {
        summary["status"] = "unknown".into();
        summary["anomalies"] =
            json!([{"kind":"pipeline_unavailable","error":"terminal_scan_failed"}]);
        return summary;
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
        // One rule for "this device is failing", shared with health's client
        // rows (convey-shell clients.rs): the capture state, and nothing else.
        // Home used to add "has any ingest rejection", so a stale device with an
        // old rejection was named on one surface and not the other (F-8).
        "failing": row.capture_state == ClientCaptureState::Degraded,
        "capture_elapsed_ms": row.capture_elapsed_ms,
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
        // The principal's id is its directory, which is what aliases map.
        &solstone_core_entities::edge_aliases(context.journal_root()),
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
    json!({"state":projection.aggregate_state,"headline":view.headline,"reason_code":projection.reason_code,"reason_text":view.reason_text,"failing_component":view.failing_component,"action":brain_action(&projection.aggregate_state, projection.reason_code.as_deref()),"identity":{"lane":projection.active_lane,"provider":projection.active_provider,"model":projection.active_model},"evidence":{"observed_at":view.evidence.observed_at,"age_seconds":view.evidence.age_seconds,"age_text":view.evidence.age_text},"components":{"generate":brain_component(inspection.record.as_ref(), "generate")},"progressing":view.progressing})
}

fn day_root(context: &HomeContext, day: &str) -> std::path::PathBuf {
    context.journal_root().join("chronicle").join(day)
}

/// Read validated structured think diagnostics from the canonical oplog namespace.
///
/// Census failures remain distinct from an empty history. Malformed JSON rows
/// are skipped; callers decide whether missing optional projections are useful.
type ThinkLogRows = Vec<(String, Vec<Map<String, Value>>)>;

fn think_oplogs(context: &HomeContext, day: &str) -> Result<ThinkLogRows, ()> {
    let day_key = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| ())?;
    let root = JournalRoot::open(context.journal_root()).map_err(|_| ())?;
    fold_oplogs(
        root,
        &[day_key],
        |result: &mut ThinkLogRows, entry, file| {
            let name = entry.name();
            if name.source().display_slug() != "think" || name.format() != OplogFormat::Jsonl {
                return Ok(());
            }
            let mut text = String::new();
            file.read_to_string(&mut text)?;
            let rows = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|value| value.as_object().cloned())
                .collect();
            result.push((name.run().display_slug().to_owned(), rows));
            Ok(())
        },
    )
    .map_err(|_| ())
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
        window: None,
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
    json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
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

pub(crate) fn transcription_processing_issue(
    journal: &Path,
    config: &Map<String, Value>,
    active_lane: Option<&str>,
) -> Option<Value> {
    let view = solstone_core_thinking::brain::applicable_transcription_verification(
        journal,
        config,
        active_lane,
    )?;
    if view.state != "failed" && view.state != "unreachable" {
        return None;
    }
    let text = solstone_core_brain::processing_headline_for_reason(&view.reason)?;
    Some(json!({
        "text": text,
        "severity": "amber",
        "href": "/app/thinking/#main",
    }))
}

/// Resolve the owner-voice tier for the journal root, degrading gracefully on error.
pub fn resolve_owner_voice_tier(context: &HomeContext) -> Option<OwnerTierOutcome> {
    match resolve_owner_tier(context.journal_root()) {
        Ok(outcome) => Some(outcome),
        Err(error) => {
            log::error!("failed to resolve owner tier for pulse: {error}");
            None
        }
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
        // Pin the day coordinate so these expectations do not depend on the
        // host's zone.
        HomeContext::with_zone(
            root,
            Utc.with_ymd_and_hms(2026, 6, 2, 13, 0, 0).unwrap(),
            chrono_tz::Tz::UTC,
        )
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
        let now = Utc
            .with_ymd_and_hms(2026, 6, 2, 13, 17, 0)
            .unwrap()
            .fixed_offset();
        assert_eq!(
            briefing_lateness_state(now, "pending"),
            json!({"late":true,"late_hours":3})
        );
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
        // No way to think chosen: the nightly run that writes the summary is off.
        assert_eq!(
            load_backlog_source(&context).validity,
            BacklogValidity::NotYet(solstone_core_system_health::NotYet::AwaitingEngine)
        );
        // A way to think, and the journal's first night long past: missing is missing.
        write(
            root.path(),
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local"}}}"#,
        );
        fs::create_dir_all(root.path().join("chronicle/20200101")).unwrap();
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
            "chronicle/20260601/talents/morning_briefing.json",
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

    /// Two capture streams, one facet, one stretch: the desktop and the
    /// terminal on 2026-06-02 (today) and 2026-06-01 (yesterday).
    fn seed_concurrent_streams(root: &std::path::Path, day: &str) {
        for (stream, key) in [
            ("device_2", "120000_300"),
            ("device_2", "120500_300"),
            ("extro_tmux", "120100_300"),
            ("extro_tmux", "120600_300"),
        ] {
            fs::create_dir_all(root.join("chronicle").join(day).join(stream).join(key)).unwrap();
        }
        for (stream, cid, source) in [
            (
                "device_2",
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "",
            ),
            (
                "extro_tmux",
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                "tmux",
            ),
        ] {
            write(
                root,
                &format!("streams/{stream}.json"),
                &json!({"name": stream, "kind": "observer", "host": null, "platform": null,
                        "created_at": 1, "last_day": day, "last_segment": null, "seq": 1,
                        "cid": cid, "source": source})
                .to_string(),
            );
        }
        let reported = |device_type: &str| {
            json!({"protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                   "reported": {"name": "fedora", "platform": "linux", "device_type": device_type,
                                "app_id": null, "app_version": null}})
        };
        write(
            root,
            "link/client-descriptions.json",
            &json!({
                "sha256:1111111111111111111111111111111111111111111111111111111111111111": reported("desktop"),
                "sha256:2222222222222222222222222222222222222222222222222222222222222222": reported("terminal"),
            })
            .to_string(),
        );
    }

    #[test]
    fn today_names_the_source_of_each_half_of_a_concurrent_pair_and_keeps_both() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        let recent = context.now_ms() - 1;
        seed_concurrent_streams(root.path(), "20260602");
        write(root.path(), "facets/work/facet.json", "{}");
        write(
            root.path(),
            "facets/work/activities/20260602.jsonl",
            &format!(
                r#"{{"id":"email_120000_300","source":"cogitate","created_at":{recent},"segments":["120000_300","120500_300"],"description":"Answered the venue thread."}}
{{"id":"terminal_120100_300","source":"cogitate","created_at":{recent},"segments":["120100_300","120600_300"],"description":"Ran the release checks."}}
{{"id":"meeting_090000_300","source":"cogitate","created_at":{recent},"segments":["090000_300"],"description":"Earlier, alone."}}"#
            ),
        );
        let rows = collect_activities(&context, "20260602");
        assert_eq!(rows.len(), 3, "nothing merged or hidden across streams");
        let label = |id: &str| {
            rows.iter()
                .find(|row| row["id"] == id)
                .and_then(|row| row.get("source_label").cloned())
        };
        assert_eq!(
            label("email_120000_300"),
            Some(crate::sources::source_phrase("computer", None).into())
        );
        assert_eq!(
            label("terminal_120100_300"),
            Some(crate::sources::source_phrase("terminal", None).into())
        );
        assert_eq!(label("meeting_090000_300"), None);
    }

    #[test]
    fn yesterday_lines_carry_the_source_of_a_concurrent_pair() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        seed_concurrent_streams(root.path(), "20260601");
        write(root.path(), "facets/work/facet.json", "{}");
        write(
            root.path(),
            "facets/work/activities/20260601.jsonl",
            r#"{"id":"email_120000_300","source":"cogitate","created_at":1,"segments":["120000_300","120500_300"],"description":"Answered the venue thread."}
{"id":"terminal_120100_300","source":"cogitate","created_at":1,"segments":["120100_300","120600_300"],"description":"Ran the release checks."}"#,
        );
        let rows = collect_top_activities_yesterday(&context);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            let expected = if row["id"] == "email_120000_300" {
                crate::sources::source_phrase("computer", None)
            } else {
                crate::sources::source_phrase("terminal", None)
            };
            assert_eq!(row["source_label"], expected.as_str(), "{}", row["id"]);
            assert!(
                crate::formatting::format_activity_label(row).contains(&expected),
                "{}",
                crate::formatting::format_activity_label(row)
            );
        }
    }

    // Captured from GET /app/home/api/pulse on 2026-09-07: three records whose
    // segments run 18:25 to 18:40 and whose talent run wrote all of them within
    // one minute of 21:56, and the two facet copies of one terminal activity.
    #[test]
    fn activity_reader_times_rows_by_their_segments_and_collapses_facet_copies() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        let recent = context.now_ms() - 1;
        write(root.path(), "facets/personal/facet.json", "{}");
        write(root.path(), "facets/solstone/facet.json", "{}");
        write(
            root.path(),
            "facets/personal/activities/20260602.jsonl",
            &format!(
                r#"{{"id":"social_183505_302","source":"cogitate","created_at":{recent},"segments":["183505_302"],"description":"Shared observations about local weather."}}
{{"id":"meeting_182504_302","source":"cogitate","created_at":{recent},"segments":["183005_300","182504_302"],"description":"Discussed switching pickup locations."}}
{{"id":"terminal_184038_305","source":"cogitate","created_at":{recent},"segments":["184038_305"],"description":"Closed an automated sponsor session."}}"#
            ),
        );
        write(
            root.path(),
            "facets/solstone/activities/20260602.jsonl",
            &format!(
                r#"{{"id":"terminal_184038_305","source":"cogitate","created_at":{recent},"segments":["184038_305"],"description":"Monitored H100 VM boot progress."}}"#
            ),
        );
        let rows = collect_activities(&context, "20260602");
        // One activity per id, newest first by when it happened, not by when the
        // talent run wrote it — every record here shares one created_at.
        let ids = rows
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "terminal_184038_305",
                "social_183505_302",
                "meeting_182504_302"
            ]
        );
        // F-6: one shape, RFC 3339 in the journal's day coordinate (UTC here).
        assert_eq!(rows[0]["display_time"], "2026-06-02T18:40:38+00:00");
        assert_eq!(rows[1]["display_time"], "2026-06-02T18:35:05+00:00");
        // The earliest segment, not the first one listed.
        assert_eq!(rows[2]["display_time"], "2026-06-02T18:25:04+00:00");
        // The two facet copies collapse and keep both facets and both sentences.
        assert_eq!(rows[0]["facets"], json!(["personal", "solstone"]));
        assert_eq!(
            rows[0]["description"],
            "Closed an automated sponsor session. Monitored H100 VM boot progress."
        );
        assert!(rows[1].get("facets").is_none());
    }

    /// F-6: the segment-derived arm and the write-time arm are one clock, the
    /// journal's day offset. Before this they were a naive local time and a UTC
    /// instant, so two rows in one list were read six hours apart on a -06:00
    /// day and neither said which clock it meant.
    #[test]
    fn activity_display_times_share_the_journals_day_offset_on_both_arms() {
        let root = TempDir::new().unwrap();
        // 19:00Z is 13:00 on a -06:00 day.
        let context = HomeContext::with_zone(
            root.path(),
            Utc.with_ymd_and_hms(2026, 6, 2, 19, 0, 0).unwrap(),
            chrono_tz::Tz::America__Denver,
        );
        write(root.path(), "facets/work/facet.json", "{}");
        write(
            root.path(),
            "facets/work/activities/20260602.jsonl",
            concat!(
                r#"{"id":"from_segments","source":"user","created_at":1780426800000,"segments":["130000_100"],"title":"timed by its segments"}"#,
                "
",
                r#"{"id":"from_write_time","source":"user","created_at":1780426800000,"segments":["20260602-1300"],"title":"timed by when it was written"}"#,
            ),
        );
        let rows = collect_activities(&context, "20260602");
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(
                row["display_time"], "2026-06-02T13:00:00-06:00",
                "{}",
                row["id"]
            );
        }
    }

    #[test]
    fn activity_reader_falls_back_to_the_write_time_without_a_parseable_segment() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        write(root.path(), "facets/work/facet.json", "{}");
        write(
            root.path(),
            "facets/work/activities/20260602.jsonl",
            r#"{"source":"user","created_at":1780405200000,"segments":["20260602-1000"],"title":"no segment clock"}"#,
        );
        let rows = collect_activities(&context, "20260602");
        assert_eq!(rows[0]["display_time"], "2026-06-02T13:00:00+00:00");
    }

    #[test]
    fn briefing_day_mapping_handles_year_and_leap_day_without_stale_fallback() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        for (presentation, analysis) in [("20270101", "20261231"), ("20240301", "20240229")] {
            let path = root.path().join(format!(
                "chronicle/{analysis}/talents/morning_briefing.json"
            ));
            assert_eq!(morning_briefing_path(&context, presentation), Some(path));
        }
        assert!(morning_briefing_path(&context, "invalid").is_none());
        write(
            root.path(),
            "chronicle/20260531/talents/morning_briefing.json",
            r#"{"metadata":{},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        );
        assert!(
            load_briefing(&context, "20260602").is_none(),
            "an older briefing cannot fill a missing day"
        );
    }

    #[test]
    fn briefing_readers_cover_required_shape_and_guard_repairs() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        assert_eq!(
            morning_briefing_path(&context, "20260602"),
            Some(
                root.path()
                    .join("chronicle/20260601/talents/morning_briefing.json")
            )
        );
        assert_eq!(
            briefing_freshness(&context, "20260602"),
            json!({"exists":false,"valid":false,"generated_label":null})
        );
        write(
            root.path(),
            "chronicle/20260601/talents/morning_briefing.json",
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
            briefing_meeting_count(
                &json!({"your_day":[{"start":"","end":""},{"start":"10:00","end":"10:00"}]})
            ),
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
        let now = Utc
            .with_ymd_and_hms(2026, 6, 2, 13, 0, 0)
            .unwrap()
            .fixed_offset();
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
                Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0)
                    .unwrap()
                    .fixed_offset(),
                "pending"
            ),
            json!({"late":false,"late_hours":0})
        );
    }

    #[test]
    fn backlog_readers_distinguish_missing_malformed_and_value_cases() {
        assert!(stuck_day_rows(&Map::new()).is_empty());
        assert_eq!(
            stuck_day_rows(
                &serde_json::from_str(r#"{"days":[{"state":"stuck"},{"state":"ready"}]}"#).unwrap()
            ),
            vec![json!({"state":"stuck"})]
        );
    }

    #[test]
    fn attention_readers_reject_malformed_records_and_use_recent_imports() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
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
            "import complete."
        );
    }

    #[test]
    fn attention_reads_the_recorded_local_wall_time_of_a_finished_import() {
        let root = TempDir::new().unwrap();
        // 13:00 UTC is 07:00 at UTC-6.
        let context = HomeContext::with_zone(
            root.path(),
            Utc.with_ymd_and_hms(2026, 6, 2, 13, 0, 0).unwrap(),
            chrono_tz::Tz::America__Denver,
        );
        let recent = json!({"imports":{"last_completed":"20260602T06:30:00","last_result_summary":"12 notes"}});
        assert_eq!(
            resolve_attention(&context, &recent).unwrap()["placeholder_text"],
            "import complete."
        );
        // An hour and a half ago locally is no longer recent.
        let old = json!({"imports":{"last_completed":"20260602T05:30:00","last_result_summary":"12 notes"}});
        assert_eq!(resolve_attention(&context, &old), None);
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
    fn awareness_reader_covers_absent_malformed_and_value_cases() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
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
            transport_refusal: None,
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
            json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
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
        assert!(pulse.window.is_none());
        let window = json!({"segments": 2, "activities": 1, "input_segments": 10, "input_activities": 4, "since_ms": 1780405100000_i64, "gaps": ["missing source"]});
        write(
            root.path(),
            "chronicle/20260602/talents/pulse.jsonl",
            &json!({"ts":1780405200000_i64,"full_details":"details","window":window}).to_string(),
        );
        let projected = load_pulse_narrative(&context, "20260602").window.unwrap();
        assert_eq!(serde_json::to_value(projected).unwrap(), window);
        write(
            root.path(),
            "chronicle/20260602/talents/pulse.jsonl",
            &json!({"ts":1780405200000_i64,"full_details":"details","window":{"segments":-1}})
                .to_string(),
        );
        assert!(load_pulse_narrative(&context, "20260602").window.is_none());
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
            "chronicle/20260601/talents/morning_briefing.json",
            "bad",
        );
        assert!(load_briefing(&context, "20260602").is_none());
        write(
            root.path(),
            "chronicle/20260601/talents/morning_briefing.json",
            r#"{"metadata":{}}"#,
        );
        assert!(load_briefing(&context, "20260602").is_none());
    }

    #[test]
    fn briefing_renderer_omits_empty_sections_and_formats_all_section_kinds() {
        let rendered = render_briefing_sections(
            &json!({"yesterday":[""],"forward_look":["next"],"your_day":[{"start":"9:00","end":"9:00","text":"meeting"}],"reading":[{"facet":"work","summary":"read"}],"needs_attention":[{"text":"act"}]}),
        );
        assert!(!rendered.contains_key("yesterday"));
        assert_eq!(rendered["forward_look"], "- next");
        assert_eq!(rendered["your_day"], "- **9:00**: meeting");
        assert_eq!(rendered["reading"], "- **work**: read");
        assert_eq!(rendered["needs_attention"], "- act");
    }

    #[test]
    fn your_day_renders_a_window_and_treats_end_only_as_a_point() {
        let rendered = render_briefing_sections(&json!({"your_day":[
            {"start":"13:00","end":"13:30","text":"Team sync."},
            {"start":"","end":"14:00","text":"Odd shape."},
            {"start":"","end":"","text":"No fixed time."}
        ]}));
        assert_eq!(
            rendered["your_day"],
            "- **13:00–13:30**: Team sync.\n- **14:00**: Odd shape.\n- No fixed time."
        );
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
    fn the_connections_card_counts_a_merged_entity_under_its_survivor() {
        let root = TempDir::new().unwrap();
        let context = context(root.path());
        for (dir, name) in [("me", "Me"), ("ada", "Ada Lovelace")] {
            write(
                root.path(),
                &format!("entities/{dir}/entity.json"),
                &json!({"id": dir, "name": name}).to_string(),
            );
        }
        write(
            root.path(),
            "logs/entity-merges.jsonl",
            "{\"source_id\":\"ada_old\",\"target_id\":\"ada\"}\n",
        );
        let connection = solstone_core_indexer_store::db::open_index(root.path()).unwrap();
        for (dst, day, path) in [("ada_old", "20260530", "a"), ("ada", "20260601", "b")] {
            connection
                .execute(
                    "INSERT INTO edges(src,dst,kind,directed,src_name,dst_name,day,facet,source,path,anchor,label,ts,weight) VALUES('me',?,'works-with',0,NULL,NULL,?,'work','test',?,NULL,NULL,1,1)",
                    rusqlite::params![dst, day, path],
                )
                .unwrap();
        }
        drop(connection);
        let network = load_connections_network(&context, &json!({"id":"me"}))
            .unwrap()
            .unwrap();
        assert_eq!(network.total_neighbors, 1);
        assert_eq!(network.neighbors[0].entity_id, "ada");
        assert_eq!(network.neighbors[0].count, 2);
        assert_eq!(network.neighbors[0].name.as_deref(), Some("Ada Lovelace"));
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
            json!({"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable","reason_text":"brain record unavailable","failing_component":null,"action":{"label":"check again","refresh":true},"identity":{"lane":null,"provider":null,"model":null},"evidence":{"observed_at":null,"age_seconds":null,"age_text":null},"components":{"generate":{"status":null,"reason_code":null,"reason_text":"unknown","observed_at":null}},"progressing":false})
        );
    }

    fn spp_ready_evidence(now: chrono::DateTime<chrono::Utc>) -> Value {
        let observed = now.to_rfc3339();
        let expires = (now + chrono::Duration::hours(2)).to_rfc3339();
        json!({
            "configuration": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "generate": {"status": "ok", "observed_at": observed, "expires_at": expires},
            "lane_prerequisites": {
                "status": "ok",
                "observed_at": observed,
                "expires_at": expires,
            }
        })
    }

    #[test]
    fn build_brain_snapshot_surfaces_confidential_attestation_refusal() {
        let config_json = json!({
            "services": {
                "confidential": {
                    "device": "abc",
                    "endpoint_url": "http://127.0.0.1:9099",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "local", "model": "served"},
                "local": {"endpoint_url": "http://127.0.0.1:9099", "served_model_id": "served", "credential": "endpoint-credential"}
            }
        });
        let config_map = config_json.as_object().unwrap().clone();

        // 1. Ready journal: gateway_unreachable -> blocked / attestation_not_verified
        let ready_temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let ready_path = ready_temp.path();
        write(ready_path, "config/journal.json", &config_json.to_string());
        solstone_core_brain::generate_fingerprint_key(ready_path).unwrap();

        let now = Utc::now();
        let ready_permit = solstone_core_brain::begin_refresh(
            ready_path,
            now,
            Some("ready-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds")
        .expect("permit exists");

        let finish_res = solstone_core_brain::finish_refresh(
            ready_path,
            ready_permit,
            spp_ready_evidence(now),
            now,
            None,
        );
        assert!(finish_res.is_ok());

        solstone_core_brain::record_confidential_attestation_refusal(
            ready_path,
            &config_map,
            "gateway_unreachable",
        );

        // The refusal is stamped by the wall clock when it is written, and a
        // record stamped after the read time projects as invalid. Read after
        // the write, not one second after the test began: the fingerprint
        // work above can take longer than that on a loaded host.
        let now_check = Utc::now() + Duration::seconds(1);
        let context = HomeContext::with_zone(ready_path, now_check, chrono_tz::Tz::UTC);
        let snapshot = build_brain_snapshot(&context);
        assert_eq!(snapshot["state"], "blocked");
        assert_eq!(snapshot["reason_code"], "attestation_not_verified");
        assert_eq!(snapshot["identity"]["lane"], "spp");

        let backlog = BacklogSource {
            backlog: None,
            validity: BacklogValidity::Valid,
            generated_at: None,
        };

        let glance = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &snapshot,
            now_check,
            None,
        );
        let issues = glance["issues"].as_array().expect("issues array");
        let headline = snapshot["headline"]
            .as_str()
            .expect("processing headline")
            .trim();
        assert!(!headline.is_empty());
        assert!(
            issues.iter().any(|issue| issue["text"] == headline),
            "the processing issue is on the glance"
        );

        // 2. Mid-check journal: certificate_invalid -> unhealthy / attestation_rejected
        let mid_check_temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let mid_check_path = mid_check_temp.path();
        write(
            mid_check_path,
            "config/journal.json",
            &config_json.to_string(),
        );
        solstone_core_brain::generate_fingerprint_key(mid_check_path).unwrap();

        let _mid_check_permit = solstone_core_brain::begin_refresh(
            mid_check_path,
            now,
            Some("mid-check-run".to_owned()),
            None,
            false,
            None,
        )
        .expect("begin_refresh succeeds")
        .expect("permit exists");

        solstone_core_brain::record_confidential_attestation_refusal(
            mid_check_path,
            &config_map,
            "certificate_invalid",
        );
        let now_check = Utc::now() + Duration::seconds(1);
        let context2 = HomeContext::with_zone(mid_check_path, now_check, chrono_tz::Tz::UTC);
        let snapshot2 = build_brain_snapshot(&context2);
        assert_eq!(snapshot2["state"], "unhealthy");
        assert_eq!(snapshot2["reason_code"], "attestation_rejected");

        let glance2 = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &snapshot2,
            now_check,
            None,
        );
        let issues2 = glance2["issues"].as_array().expect("issues array");
        let headline = snapshot2["headline"]
            .as_str()
            .expect("processing headline")
            .trim();
        assert!(!headline.is_empty());
        assert!(
            issues2.iter().any(|issue| issue["text"] == headline),
            "the processing issue is on the glance"
        );
    }

    #[test]
    fn transcription_processing_issue_and_health_glance() {
        let config_json = json!({
            "services": {
                "confidential": {
                    "device": "abc",
                    "endpoint_url": "http://127.0.0.1:9099",
                    "served_model_id": "served",
                    "credential_fingerprint_sha256": "cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a"
                }
            },
            "providers": {
                "active": {"provider": "openai", "model": "gpt-4o"},
                "local": {
                    "endpoint_url": "http://127.0.0.1:9099",
                    "served_model_id": "served",
                    "credential": "endpoint-credential"
                }
            }
        });
        let config_map = config_json.as_object().unwrap().clone();
        let now = Utc::now();
        let backlog = BacklogSource {
            backlog: Some(json!({"stuck_days": 0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some(now.to_rfc3339()),
        };
        let ready_brain = json!({
            "state": "ready",
            "progressing": false
        });

        // 1. certificate_invalid -> 1 issue with href: /app/thinking/#main and headline for attestation_rejected
        let temp1 = tempfile::tempdir_in("/var/tmp").unwrap();
        let path1 = temp1.path();
        solstone_core_brain::record_transcription_verification(
            path1,
            "certificate_invalid",
            "http://127.0.0.1:9099",
        )
        .unwrap();

        let issue1 = transcription_processing_issue(path1, &config_map, Some("byo-cloud"));
        assert!(issue1.is_some());
        let expected_headline1 =
            solstone_core_brain::processing_headline_for_reason("attestation_rejected").unwrap();
        assert_eq!(issue1.as_ref().unwrap()["text"], expected_headline1);
        assert_eq!(issue1.as_ref().unwrap()["href"], "/app/thinking/#main");

        let glance1 = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &ready_brain,
            now,
            issue1.as_ref(),
        );
        let issues1 = glance1["issues"].as_array().expect("issues array");
        assert_eq!(issues1.len(), 1);
        assert_eq!(issues1[0]["href"], "/app/thinking/#main");
        assert_eq!(issues1[0]["text"], expected_headline1);

        // 2. gateway_unreachable -> text matching attestation_not_verified, href: /app/thinking/#main
        let temp2 = tempfile::tempdir_in("/var/tmp").unwrap();
        let path2 = temp2.path();
        solstone_core_brain::record_transcription_verification(
            path2,
            "gateway_unreachable",
            "http://127.0.0.1:9099",
        )
        .unwrap();

        let issue2 = transcription_processing_issue(path2, &config_map, Some("byo-cloud"));
        assert!(issue2.is_some());
        let expected_headline2 =
            solstone_core_brain::processing_headline_for_reason("attestation_not_verified")
                .unwrap();
        assert_eq!(issue2.as_ref().unwrap()["text"], expected_headline2);
        assert_eq!(issue2.as_ref().unwrap()["href"], "/app/thinking/#main");

        let glance2 = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &ready_brain,
            now,
            issue2.as_ref(),
        );
        let issues2 = glance2["issues"].as_array().expect("issues array");
        assert_eq!(issues2.len(), 1);
        assert_eq!(issues2[0]["href"], "/app/thinking/#main");
        assert_eq!(issues2[0]["text"], expected_headline2);

        // 3. Stored retired code -> stale -> no issue with href /app/thinking/#main
        let temp3 = tempfile::tempdir_in("/var/tmp").unwrap();
        let path3 = temp3.path();
        let tv_path3 = solstone_core_brain::transcription_verification_path(path3);
        std::fs::create_dir_all(tv_path3.parent().unwrap()).unwrap();
        std::fs::write(
            &tv_path3,
            serde_json::to_vec(&serde_json::json!({
                "reason": "nvattest_install_in_progress",
                "observed_at": "2026-01-01T00:00:00Z",
                "endpoint": "http://127.0.0.1:9099",
            }))
            .unwrap(),
        )
        .unwrap();

        let issue3 = transcription_processing_issue(path3, &config_map, Some("byo-cloud"));
        assert!(issue3.is_none());

        let glance3 = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &ready_brain,
            now,
            issue3.as_ref(),
        );
        let issues3 = glance3["issues"].as_array().expect("issues array");
        assert!(
            !issues3
                .iter()
                .any(|i| i.get("href").and_then(Value::as_str) == Some("/app/thinking/#main"))
        );

        // 4. Journal with missing brain record (unknown state snapshot) + certificate_invalid -> glance has no thinking-href issue; snapshot headline appears once
        let temp4 = tempfile::tempdir_in("/var/tmp").unwrap();
        let path4 = temp4.path();
        write(path4, "config/journal.json", &config_json.to_string());
        let context4 = HomeContext::with_zone(path4, now, chrono_tz::Tz::UTC);
        let snapshot4 = build_brain_snapshot(&context4);
        let headline4 = snapshot4["headline"]
            .as_str()
            .expect("processing headline")
            .trim();
        assert!(!headline4.is_empty());

        let glance4 = crate::health_glance::build_health_glance(
            &json!({}),
            None,
            &backlog,
            &snapshot4,
            now,
            issue1.as_ref(),
        );
        let issues4 = glance4["issues"].as_array().expect("issues array");
        assert!(
            !issues4
                .iter()
                .any(|i| i.get("href").and_then(Value::as_str) == Some("/app/thinking/#main"))
        );
        assert_eq!(
            issues4
                .iter()
                .filter(|i| i.get("text").and_then(Value::as_str) == Some(headline4))
                .count(),
            1
        );

        // 5. Malformed status file or missing file -> no thinking-href issue
        let temp5 = tempfile::tempdir_in("/var/tmp").unwrap();
        let path5 = temp5.path();
        // Missing file
        assert!(transcription_processing_issue(path5, &config_map, Some("byo-cloud")).is_none());

        // Array
        write(path5, "health/confidential-transcription.json", "[]");
        assert!(transcription_processing_issue(path5, &config_map, Some("byo-cloud")).is_none());

        // Raw string
        write(
            path5,
            "health/confidential-transcription.json",
            "\"raw string\"",
        );
        assert!(transcription_processing_issue(path5, &config_map, Some("byo-cloud")).is_none());

        // Empty fields
        write(
            path5,
            "health/confidential-transcription.json",
            r#"{"reason":"","endpoint":""}"#,
        );
        assert!(transcription_processing_issue(path5, &config_map, Some("byo-cloud")).is_none());
    }
}
