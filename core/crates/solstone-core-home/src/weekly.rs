// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::HomeContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeekJudgment {
    Page(TrustedWeek),
    CantShow,
    CouldntCheck,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedWeek {
    pub start_day: String,
    pub end_day: String,
    pub days: Vec<TrustedDay>,
    pub memories: Vec<TrustedMemory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDay {
    pub day: String,
    pub state: String,
    pub memory_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedMemory {
    pub id: String,
    pub key: String,
    pub day: String,
    pub text: String,
    pub source: TrustedSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedSource {
    pub kind: String,
    pub uri: String,
    pub briefing_day: Option<String>,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftOut {
    None,
    Keys(BTreeSet<String>),
    Unreadable,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLeftOut {
    keys: Vec<String>,
}

pub fn left_out_path(journal: &Path) -> PathBuf {
    journal.join("health/week-left-out.json")
}

pub fn week_day(segment: &str) -> Option<NaiveDate> {
    if segment.len() != 8 || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    NaiveDate::parse_from_str(segment, "%Y%m%d").ok()
}

pub fn judge(journal: &Path, day: &str) -> WeekJudgment {
    let Some(start_date) = week_day(day) else {
        return WeekJudgment::CouldntCheck;
    };

    let json_path = journal.join(format!("reflections/weekly/{day}.json"));
    let content = match fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let md_path = journal.join(format!("reflections/weekly/{day}.md"));
            return match fs::metadata(&md_path) {
                Ok(meta) if meta.is_file() => WeekJudgment::CantShow,
                Ok(_) => WeekJudgment::Absent,
                Err(e) if e.kind() == ErrorKind::NotFound => WeekJudgment::Absent,
                Err(_) => WeekJudgment::CouldntCheck,
            };
        }
        Err(_) => return WeekJudgment::CouldntCheck,
    };

    if start_date.weekday() != chrono::Weekday::Sun {
        return WeekJudgment::CouldntCheck;
    }

    let value: Value = match serde_json::from_str(&content) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => return WeekJudgment::CouldntCheck,
    };

    let Some(version_val) = value.get("version") else {
        return WeekJudgment::CouldntCheck;
    };

    if version_val.as_i64() != Some(1) {
        return WeekJudgment::CantShow;
    }

    let mut trusted_memories = Vec::new();
    let mut memory_ids = BTreeSet::new();

    if let Some(mem_val) = value.get("memories") {
        let Some(mem_arr) = mem_val.as_array() else {
            return WeekJudgment::CouldntCheck;
        };
        trusted_memories.reserve(mem_arr.len());
        for m in mem_arr {
            let (Some(id), Some(key), Some(mem_day), Some(text), Some(source_obj)) = (
                m.get("id").and_then(Value::as_str),
                m.get("key").and_then(Value::as_str),
                m.get("day").and_then(Value::as_str),
                m.get("text").and_then(Value::as_str),
                m.get("source").and_then(Value::as_object),
            ) else {
                return WeekJudgment::CouldntCheck;
            };

            if id.is_empty() || key.is_empty() || mem_day.is_empty() || text.is_empty() {
                return WeekJudgment::CouldntCheck;
            }

            if week_day(mem_day).is_none() {
                return WeekJudgment::CouldntCheck;
            }

            let (Some(kind), Some(uri)) = (
                source_obj.get("kind").and_then(Value::as_str),
                source_obj.get("uri").and_then(Value::as_str),
            ) else {
                return WeekJudgment::CouldntCheck;
            };

            if kind != "briefing" {
                return WeekJudgment::CantShow;
            }

            if !memory_ids.insert(id.to_string()) {
                return WeekJudgment::CouldntCheck;
            }

            let briefing_day = source_obj
                .get("briefing_day")
                .and_then(Value::as_str)
                .map(String::from);

            let mut refs = Vec::new();
            if let Some(refs_arr) = source_obj.get("refs").and_then(Value::as_array) {
                for r in refs_arr {
                    if let Some(r_str) = r.as_str() {
                        refs.push(r_str.to_string());
                    }
                }
            }

            trusted_memories.push(TrustedMemory {
                id: id.to_string(),
                key: key.to_string(),
                day: mem_day.to_string(),
                text: text.to_string(),
                source: TrustedSource {
                    kind: kind.to_string(),
                    uri: uri.to_string(),
                    briefing_day,
                    refs,
                },
            });
        }
    }

    let Some(days_arr) = value.get("days").and_then(Value::as_array) else {
        return WeekJudgment::CouldntCheck;
    };

    if days_arr.len() != 7 {
        return WeekJudgment::CouldntCheck;
    }

    let memory_day_by_id: std::collections::BTreeMap<&str, &str> = trusted_memories
        .iter()
        .map(|m| (m.id.as_str(), m.day.as_str()))
        .collect();

    let mut referenced_memory_ids = BTreeSet::new();
    let mut trusted_days = Vec::with_capacity(7);

    for (i, d) in days_arr.iter().enumerate() {
        let expected_date = start_date + Duration::days(i as i64);
        let expected_day_str = expected_date.format("%Y%m%d").to_string();

        let (Some(day_str), Some(state)) = (
            d.get("day").and_then(Value::as_str),
            d.get("state").and_then(Value::as_str),
        ) else {
            return WeekJudgment::CouldntCheck;
        };

        if day_str != expected_day_str {
            return WeekJudgment::CouldntCheck;
        }

        let memory_id = d.get("memory_id").and_then(Value::as_str);

        match state {
            "memory" => {
                let Some(mem_id) = memory_id else {
                    return WeekJudgment::CouldntCheck;
                };
                let Some(mem_day) = memory_day_by_id.get(mem_id) else {
                    return WeekJudgment::CouldntCheck;
                };
                if *mem_day != day_str {
                    return WeekJudgment::CouldntCheck;
                }
                if !referenced_memory_ids.insert(mem_id) {
                    return WeekJudgment::CouldntCheck;
                }
            }
            "nothing_shared" | "unreadable" | "not_ready" => {}
            _ => return WeekJudgment::CouldntCheck,
        }

        trusted_days.push(TrustedDay {
            day: day_str.to_string(),
            state: state.to_string(),
            memory_id: memory_id.map(String::from),
        });
    }

    let start_day = value
        .get("week")
        .and_then(|w| w.get("start"))
        .and_then(Value::as_str)
        .unwrap_or(day)
        .to_string();
    let end_day = value
        .get("week")
        .and_then(|w| w.get("end"))
        .and_then(Value::as_str)
        .unwrap_or(day)
        .to_string();

    WeekJudgment::Page(TrustedWeek {
        start_day,
        end_day,
        days: trusted_days,
        memories: trusted_memories,
    })
}

pub fn has_page(journal: &Path, day: &str) -> bool {
    if week_day(day).is_none() {
        return false;
    }
    matches!(judge(journal, day), WeekJudgment::Page(_))
}

pub fn read_left_out(journal: &Path) -> LeftOut {
    let path = left_out_path(journal);
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => return LeftOut::None,
        Err(_) => return LeftOut::Unreadable,
    };
    match serde_json::from_str::<RawLeftOut>(&content) {
        Ok(raw) => LeftOut::Keys(raw.keys.into_iter().collect()),
        Err(_) => LeftOut::Unreadable,
    }
}

pub fn left_out_bytes(keys: &BTreeSet<String>) -> Vec<u8> {
    let sorted_keys: Vec<String> = keys.iter().cloned().collect();
    serde_json::to_vec(&RawLeftOut { keys: sorted_keys }).expect("left out serialize")
}

// Private lowercase date formatting helpers
fn month_name(month: u32) -> &'static str {
    match month {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => "",
    }
}

fn weekday_name(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "monday",
        chrono::Weekday::Tue => "tuesday",
        chrono::Weekday::Wed => "wednesday",
        chrono::Weekday::Thu => "thursday",
        chrono::Weekday::Fri => "friday",
        chrono::Weekday::Sat => "saturday",
        chrono::Weekday::Sun => "sunday",
    }
}

fn short_weekday(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "mon",
        chrono::Weekday::Tue => "tue",
        chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu",
        chrono::Weekday::Fri => "fri",
        chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    }
}

fn format_week_title(date: NaiveDate, current_year: i32) -> String {
    let month = month_name(date.month());
    let day = date.day();
    if date.year() == current_year {
        format!("week of {month} {day}")
    } else {
        format!("week of {month} {day}, {}", date.year())
    }
}

fn format_unreadable_line(date: NaiveDate, current_year: i32) -> String {
    let month = month_name(date.month());
    let day = date.day();
    if date.year() == current_year {
        format!("the week of {month} {day} couldn't be read.")
    } else {
        format!(
            "the week of {month} {day}, {}, couldn't be read.",
            date.year()
        )
    }
}

fn format_days_conjunction(days: &[&str]) -> String {
    match days.len() {
        0 => String::new(),
        1 => days[0].to_string(),
        2 => format!("{} and {}", days[0], days[1]),
        _ => {
            let (last, rest) = days.split_last().unwrap();
            format!("{} and {}", rest.join(", "), last)
        }
    }
}

fn encode_uri_ref(uri: &str) -> String {
    let mut encoded = String::with_capacity(uri.len() * 3);
    for byte in uri.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

pub fn page_model(
    journal: &Path,
    day: &str,
    current_year: i32,
    is_moment: &dyn Fn(&str) -> bool,
) -> Result<Value, WeekJudgment> {
    let trusted = match judge(journal, day) {
        WeekJudgment::Page(t) => t,
        other => return Err(other),
    };

    let start_date = week_day(day).ok_or(WeekJudgment::CouldntCheck)?;
    let title = format_week_title(start_date, current_year);

    let left_out = read_left_out(journal);
    let left_out_notice = match &left_out {
        LeftOut::Unreadable => {
            Some("your left-out memories couldn't be checked, so everything is showing.")
        }
        _ => None,
    };
    let empty_set = BTreeSet::new();
    let left_out_keys = match &left_out {
        LeftOut::Keys(keys) => keys,
        _ => &empty_set,
    };

    let mut rows = Vec::new();
    let mut shown_count = 0_usize;
    let mut memory_by_id = std::collections::BTreeMap::new();
    for m in &trusted.memories {
        memory_by_id.insert(m.id.as_str(), m);
    }

    for d in &trusted.days {
        if d.state != "memory" {
            continue;
        }
        if let Some(mem) = d
            .memory_id
            .as_deref()
            .and_then(|id| memory_by_id.get(id).copied())
        {
            if left_out_keys.contains(&mem.key) {
                rows.push(json!({
                    "left_out": true,
                    "key": mem.key,
                }));
            } else {
                shown_count += 1;
                let mem_date = week_day(&mem.day).unwrap_or(start_date);
                let day_label = format!("{} {}", short_weekday(mem_date), mem_date.day());

                let (peek_ref, peek_label) = mem
                    .source
                    .refs
                    .iter()
                    .find(|r| is_moment(r.as_str()))
                    .map(|r| (r.as_str(), "open that moment →"))
                    .unwrap_or((mem.source.uri.as_str(), "open it in thinking →"));

                let peek_href = format!("/source?ref={}", encode_uri_ref(peek_ref));
                let source_href = format!("/source?ref={}", encode_uri_ref(&mem.source.uri));
                let menu_href = peek_href.clone();

                let briefing_date = mem
                    .source
                    .briefing_day
                    .as_deref()
                    .and_then(week_day)
                    .unwrap_or(mem_date);
                let presentation_date = briefing_date + Duration::days(1);
                let peek_caption = format!(
                    "from the morning briefing on {} {}",
                    short_weekday(presentation_date),
                    presentation_date.day()
                );

                rows.push(json!({
                    "key": mem.key,
                    "text": mem.text,
                    "day_label": day_label,
                    "source_href": source_href,
                    "peek_href": peek_href,
                    "peek_label": peek_label,
                    "peek_caption": peek_caption,
                    "menu_href": menu_href,
                    "left_out": false,
                }));
            }
        }
    }

    let intro = match shown_count {
        0 => "nothing from this week is on this page.".to_string(),
        1 => "your week, a memory from one day.".to_string(),
        n => format!("your week, a memory from each of {n} days."),
    };

    let mut present_states = BTreeSet::new();
    let mut cells = Vec::with_capacity(7);
    let mut memory_day_names = Vec::new();
    let mut unreadable_day_names = Vec::new();
    let mut not_ready_day_names = Vec::new();

    for d in &trusted.days {
        let date = week_day(&d.day).unwrap_or(start_date);
        let weekday = weekday_name(date);
        let day_num = date.day();

        let cell_state = if d.state == "memory" {
            let is_left_out = d
                .memory_id
                .as_deref()
                .and_then(|id| memory_by_id.get(id))
                .is_some_and(|m| left_out_keys.contains(&m.key));
            if is_left_out { "not_on_page" } else { "memory" }
        } else {
            d.state.as_str()
        };

        present_states.insert(cell_state);

        let phrase = match cell_state {
            "memory" => {
                memory_day_names.push(weekday);
                "a memory from this day"
            }
            "not_on_page" => "in your journal, not on this page",
            "nothing_shared" => "nothing in your journal",
            "unreadable" => {
                unreadable_day_names.push(weekday);
                "couldn't be read"
            }
            "not_ready" => {
                not_ready_day_names.push(weekday);
                "wasn't ready in time"
            }
            _ => "",
        };

        cells.push(json!({
            "day": d.day,
            "state": cell_state,
            "day_number": day_num.to_string(),
            "weekday": short_weekday(date),
            "accessible_name": format!("{weekday} {day_num}, {phrase}"),
        }));
    }

    let mut legend = Vec::new();
    for (state, label) in [
        ("memory", "a memory from this day"),
        ("not_on_page", "in your journal, not on this page"),
        ("nothing_shared", "nothing in your journal"),
        ("unreadable", "couldn't be read"),
        ("not_ready", "wasn't ready in time"),
    ] {
        if present_states.contains(state) {
            legend.push(json!({
                "state": state,
                "label": label,
            }));
        }
    }

    let mut from_parts = Vec::new();
    if !memory_day_names.is_empty() {
        from_parts.push(format!(
            "from {}.",
            format_days_conjunction(&memory_day_names)
        ));
    }
    if !unreadable_day_names.is_empty() {
        from_parts.push(format!(
            "something from {} couldn't be read.",
            format_days_conjunction(&unreadable_day_names)
        ));
    }
    if !not_ready_day_names.is_empty() {
        let (verb, _) = if not_ready_day_names.len() == 1 {
            ("wasn't", "")
        } else {
            ("weren't", "")
        };
        from_parts.push(format!(
            "{} {verb} ready in time.",
            format_days_conjunction(&not_ready_day_names)
        ));
    }

    let from_line = if from_parts.is_empty() {
        Value::Null
    } else {
        Value::String(from_parts.join(" "))
    };

    // Nav scan
    let mut prev_label = Value::Null;
    let mut prev_href = Value::Null;
    let mut next_label = Value::Null;
    let mut next_href = Value::Null;

    let weekly_dir = journal.join("reflections/weekly");
    if let Ok(entries) = fs::read_dir(weekly_dir) {
        let mut valid_stems = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(stem) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|stem| week_day(stem).is_some() && has_page(journal, stem))
            {
                valid_stems.push(stem.to_string());
            }
        }
        valid_stems.sort();
        valid_stems.dedup();

        if let Some(idx) = valid_stems.iter().position(|s| s == day) {
            if idx > 0 {
                let p_stem = &valid_stems[idx - 1];
                if let Some(p_date) = week_day(p_stem) {
                    prev_label =
                        Value::String(format!("← {}", format_week_title(p_date, current_year)));
                    prev_href = Value::String(format!("/app/home/week/{p_stem}"));
                }
            }
            if idx + 1 < valid_stems.len() {
                let n_stem = &valid_stems[idx + 1];
                if let Some(n_date) = week_day(n_stem) {
                    next_label =
                        Value::String(format!("{} →", format_week_title(n_date, current_year)));
                    next_href = Value::String(format!("/app/home/week/{n_stem}"));
                }
            }
        }
    }

    Ok(json!({
        "title": title,
        "day": day,
        "intro": intro,
        "cells": cells,
        "legend": legend,
        "from_line": from_line,
        "rows": rows,
        "left_out_notice": left_out_notice,
        "prev_label": prev_label,
        "prev_href": prev_href,
        "next_label": next_label,
        "next_href": next_href,
        "end_line": "that's the week.",
    }))
}

pub fn card(context: &HomeContext) -> Value {
    let weekly_dir = context.journal_root().join("reflections/weekly");
    let entries = match fs::read_dir(&weekly_dir) {
        Ok(e) => e,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return first_card(context);
        }
        Err(_) => {
            return unchecked_card();
        }
    };

    let mut candidate_json_stems = Vec::new();
    let mut candidate_md_stems = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => return unchecked_card(),
        };
        let path = entry.path();
        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(_) => return unchecked_card(),
        };
        if !metadata.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|stem| week_day(stem).is_some())
        else {
            continue;
        };
        if ext == "json" {
            candidate_json_stems.push(stem.to_string());
        } else if ext == "md" {
            candidate_md_stems.push(stem.to_string());
        }
    }

    if !candidate_json_stems.is_empty() {
        candidate_json_stems.sort();
        let newest = candidate_json_stems.pop().unwrap();

        let current_year = context.local_date().year();
        let stem_date = week_day(&newest).unwrap();

        match judge(context.journal_root(), &newest) {
            WeekJudgment::Page(trusted) => {
                let title = format_week_title(stem_date, current_year);
                let url = format!("/app/home/week/{newest}");

                let left_out = read_left_out(context.journal_root());
                let empty_set = BTreeSet::new();
                let left_out_keys = match &left_out {
                    LeftOut::Keys(keys) => keys,
                    _ => &empty_set,
                };
                let memory_by_id: std::collections::BTreeMap<&str, &TrustedMemory> = trusted
                    .memories
                    .iter()
                    .map(|m| (m.id.as_str(), m))
                    .collect();
                let mut first_shown = None;
                for d in &trusted.days {
                    if d.state == "memory"
                        && let Some(mem_id) = &d.memory_id
                        && let Some(mem) = memory_by_id.get(mem_id.as_str())
                        && !left_out_keys.contains(&mem.key)
                    {
                        first_shown = Some(mem.text.clone());
                        break;
                    }
                }
                let (memory, empty) = match left_out {
                    LeftOut::Unreadable => (Value::Null, Value::Null),
                    _ => match first_shown {
                        Some(text) => (Value::String(text), Value::Null),
                        None => (
                            Value::Null,
                            Value::String("nothing from this week is showing.".to_string()),
                        ),
                    },
                };

                json!({
                    "state": "week",
                    "title": title,
                    "url": url,
                    "memory": memory,
                    "empty": empty,
                    "line": Value::Null,
                })
            }
            WeekJudgment::CantShow | WeekJudgment::CouldntCheck => {
                let line = format_unreadable_line(stem_date, current_year);
                json!({
                    "state": "unreadable",
                    "title": Value::Null,
                    "url": Value::Null,
                    "memory": Value::Null,
                    "empty": Value::Null,
                    "line": line,
                })
            }
            WeekJudgment::Absent => {
                if !candidate_md_stems.is_empty() {
                    next_week_card(context)
                } else {
                    first_card(context)
                }
            }
        }
    } else if !candidate_md_stems.is_empty() {
        next_week_card(context)
    } else {
        first_card(context)
    }
}

fn next_week_card(context: &HomeContext) -> Value {
    let line = if solstone_core_journal_config::no_thinking_engine_chosen(context.journal_root()) {
        "your next week comes once processing is set up.".to_string()
    } else {
        let weekday =
            solstone_core_system::schedule::configured_weekly_day_name(context.journal_root());
        format!("your next week comes on {weekday}.")
    };

    json!({
        "state": "first",
        "title": Value::Null,
        "url": Value::Null,
        "memory": Value::Null,
        "empty": Value::Null,
        "line": line,
    })
}

fn unchecked_card() -> Value {
    json!({
        "state": "unchecked",
        "title": Value::Null,
        "url": Value::Null,
        "memory": Value::Null,
        "empty": Value::Null,
        "line": "your week couldn't be checked.",
    })
}

fn first_card(context: &HomeContext) -> Value {
    let line = if solstone_core_journal_config::no_thinking_engine_chosen(context.journal_root()) {
        "your first week comes once processing is set up.".to_string()
    } else {
        let weekday =
            solstone_core_system::schedule::configured_weekly_day_name(context.journal_root());
        format!("your first week comes on {weekday}.")
    };

    json!({
        "state": "first",
        "title": Value::Null,
        "url": Value::Null,
        "memory": Value::Null,
        "empty": Value::Null,
        "line": line,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn valid_week_json(sunday: &str) -> String {
        let date = NaiveDate::parse_from_str(sunday, "%Y%m%d").unwrap();
        let days_json = (0..7)
            .map(|i| {
                let d = (date + Duration::days(i)).format("%Y%m%d").to_string();
                if i == 0 {
                    format!(r#"{{"day":"{d}","state":"memory","memory_id":"m0"}}"#)
                } else {
                    format!(r#"{{"day":"{d}","state":"nothing_shared"}}"#)
                }
            })
            .collect::<Vec<_>>()
            .join(",");

        format!(
            r#"{{
                "version": 1,
                "week": {{"start":"{sunday}","end":"{end}"}},
                "days": [{days_json}],
                "memories": [
                    {{
                        "id": "m0",
                        "key": "k0",
                        "day": "{sunday}",
                        "text": "Memory for sunday.",
                        "source": {{
                            "kind": "briefing",
                            "uri": "sol://chronicle/{sunday}/talents/morning_briefing",
                            "briefing_day": "{sunday}",
                            "refs": []
                        }}
                    }}
                ]
            }}"#,
            end = (date + Duration::days(6)).format("%Y%m%d")
        )
    }

    #[test]
    fn page_from_temp_copy_of_fixture() {
        let root = TempDir::new().unwrap();
        let fixture_json =
            include_str!("../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            fixture_json,
        );

        let model = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model["title"], "week of march 8");
        assert_eq!(model["intro"], "your week, a memory from each of 3 days.");
        assert_eq!(model["cells"].as_array().unwrap().len(), 7);
        assert_eq!(
            model["cells"][0]["accessible_name"],
            "sunday 8, a memory from this day"
        );
        assert_eq!(
            model["cells"][3]["accessible_name"],
            "wednesday 11, nothing in your journal"
        );

        let legend = model["legend"].as_array().unwrap();
        assert_eq!(legend.len(), 2);
        assert_eq!(legend[0]["state"], "memory");
        assert_eq!(legend[1]["state"], "nothing_shared");

        assert_eq!(model["from_line"], "from sunday, monday and tuesday.");
        assert_eq!(model["rows"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn nav_skips_missing_and_md_only_and_respects_years() {
        let root = TempDir::new().unwrap();
        let fixture_json =
            include_str!("../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
        // Week A (20250914 Sunday), Week B (20260301 md only Sunday), Week C (20260308 json Sunday)
        write(
            root.path(),
            "reflections/weekly/20250914.json",
            &valid_week_json("20250914"),
        );
        write(root.path(), "reflections/weekly/20260301.md", "# Not json");
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            fixture_json,
        );

        let model_c = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model_c["prev_label"], "← week of september 14, 2025");
        assert_eq!(model_c["prev_href"], "/app/home/week/20250914");
        assert!(model_c["next_label"].is_null());

        let model_a = page_model(root.path(), "20250914", 2026, &|_| false).unwrap();
        assert_eq!(model_a["next_label"], "week of march 8 →");
        assert_eq!(model_a["next_href"], "/app/home/week/20260308");
        assert!(model_a["prev_label"].is_null());
    }

    #[test]
    fn from_line_conjunctions_and_omission() {
        assert_eq!(format_days_conjunction(&["sunday"]), "sunday");
        assert_eq!(
            format_days_conjunction(&["sunday", "monday"]),
            "sunday and monday"
        );
        assert_eq!(
            format_days_conjunction(&["sunday", "monday", "thursday", "saturday"]),
            "sunday, monday, thursday and saturday"
        );
    }

    #[test]
    fn from_line_variants_and_all_nothing_shared() {
        let root = TempDir::new().unwrap();

        // 1 unreadable
        let mut days = (0..7)
            .map(|i| {
                let d = format!("202603{:02}", 8 + i);
                let state = if i == 1 {
                    "unreadable"
                } else {
                    "nothing_shared"
                };
                format!(r#"{{"day":"{d}","state":"{state}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json1 = format!(
            r#"{{"version":1,"week":{{"start":"20260308","end":"20260314"}},"days":[{days}],"memories":[]}}"#
        );
        write(root.path(), "reflections/weekly/20260308.json", &json1);
        let m1 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(m1["from_line"], "something from monday couldn't be read.");

        // 2 unreadable
        days = (0..7)
            .map(|i| {
                let d = format!("202603{:02}", 8 + i);
                let state = if i == 1 || i == 2 {
                    "unreadable"
                } else {
                    "nothing_shared"
                };
                format!(r#"{{"day":"{d}","state":"{state}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json2 = format!(
            r#"{{"version":1,"week":{{"start":"20260308","end":"20260314"}},"days":[{days}],"memories":[]}}"#
        );
        write(root.path(), "reflections/weekly/20260308.json", &json2);
        let m2 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(
            m2["from_line"],
            "something from monday and tuesday couldn't be read."
        );

        // 1 not ready ("wasn't")
        days = (0..7)
            .map(|i| {
                let d = format!("202603{:02}", 8 + i);
                let state = if i == 0 {
                    "not_ready"
                } else {
                    "nothing_shared"
                };
                format!(r#"{{"day":"{d}","state":"{state}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json3 = format!(
            r#"{{"version":1,"week":{{"start":"20260308","end":"20260314"}},"days":[{days}],"memories":[]}}"#
        );
        write(root.path(), "reflections/weekly/20260308.json", &json3);
        let m3 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(m3["from_line"], "sunday wasn't ready in time.");

        // 2 not ready ("weren't")
        days = (0..7)
            .map(|i| {
                let d = format!("202603{:02}", 8 + i);
                let state = if i == 0 || i == 1 {
                    "not_ready"
                } else {
                    "nothing_shared"
                };
                format!(r#"{{"day":"{d}","state":"{state}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json4 = format!(
            r#"{{"version":1,"week":{{"start":"20260308","end":"20260314"}},"days":[{days}],"memories":[]}}"#
        );
        write(root.path(), "reflections/weekly/20260308.json", &json4);
        let m4 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(m4["from_line"], "sunday and monday weren't ready in time.");

        // All nothing_shared -> from_line is null
        days = (0..7)
            .map(|i| {
                let d = format!("202603{:02}", 8 + i);
                format!(r#"{{"day":"{d}","state":"nothing_shared"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let json5 = format!(
            r#"{{"version":1,"week":{{"start":"20260308","end":"20260314"}},"days":[{days}],"memories":[]}}"#
        );
        write(root.path(), "reflections/weekly/20260308.json", &json5);
        let m5 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert!(m5["from_line"].is_null());
        assert_eq!(m5["intro"], "nothing from this week is on this page.");
    }

    #[test]
    fn leave_out_overlay_and_undo_and_unreadable_behavior() {
        let root = TempDir::new().unwrap();
        let fixture_json =
            include_str!("../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            fixture_json,
        );

        // Hide 1 memory key
        let mut keys = BTreeSet::new();
        keys.insert("b359922341f75193".to_string());
        write(
            root.path(),
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );

        let model = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model["intro"], "your week, a memory from each of 2 days.");
        assert_eq!(model["rows"][0]["left_out"], true);
        assert!(model["rows"][0].get("text").is_none());
        assert_eq!(model["cells"][0]["state"], "not_on_page");
        assert_eq!(
            model["cells"][0]["accessible_name"],
            "sunday 8, in your journal, not on this page"
        );
        assert_eq!(model["from_line"], "from monday and tuesday.");

        // All memories left out (N=0)
        keys.insert("83bb27460e2148f3".to_string());
        keys.insert("f85648d077c433ce".to_string());
        write(
            root.path(),
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );
        let model_all_out = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(
            model_all_out["intro"],
            "nothing from this week is on this page."
        );

        // Leave only 1 memory shown (N=1)
        keys.remove("83bb27460e2148f3");
        write(
            root.path(),
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );
        let model_n1 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model_n1["intro"], "your week, a memory from one day.");

        // Unreadable left-out file
        write(root.path(), "health/week-left-out.json", "garbage bytes");
        let model_unreadable = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(
            model_unreadable["left_out_notice"],
            "your left-out memories couldn't be checked, so everything is showing."
        );
        assert_eq!(model_unreadable["rows"].as_array().unwrap().len(), 3);
        assert_eq!(model_unreadable["rows"][0]["left_out"], false);

        // Key-based matching: same key with new memory ID vs changed key
        let mut key_set = BTreeSet::new();
        key_set.insert("k_shared".to_string());
        write(
            root.path(),
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&key_set)).unwrap(),
        );

        // 1. Memory has k_shared with initial id m_initial -> left out
        let json_mem_1 = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m_initial"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m_initial",
                    "key": "k_shared",
                    "day": "20260308",
                    "text": "First generation.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                }
            ]
        }"#;
        write(root.path(), "reflections/weekly/20260308.json", json_mem_1);
        let model_k1 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model_k1["rows"][0]["left_out"], true);

        // 2. Same key k_shared with new memory ID m_reprocessed -> still left out
        let json_mem_2 = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m_reprocessed"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m_reprocessed",
                    "key": "k_shared",
                    "day": "20260308",
                    "text": "Second generation.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                }
            ]
        }"#;
        write(root.path(), "reflections/weekly/20260308.json", json_mem_2);
        let model_k2 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model_k2["rows"][0]["left_out"], true);

        // 3. Changed key k_different with same or new ID -> not left out
        let json_mem_3 = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m_reprocessed"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m_reprocessed",
                    "key": "k_different",
                    "day": "20260308",
                    "text": "Different memory.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                }
            ]
        }"#;
        write(root.path(), "reflections/weekly/20260308.json", json_mem_3);
        let model_k3 = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model_k3["rows"][0]["left_out"], false);
    }

    #[test]
    fn card_states_and_schedule_day_and_engine_setting() {
        let root = TempDir::new().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap();
        let ctx = HomeContext::with_zone(root.path(), now, chrono_tz::Tz::UTC);

        // 1. empty directory and no engine
        assert_eq!(card(&ctx)["state"], "first");
        assert_eq!(
            card(&ctx)["line"],
            "your first week comes once processing is set up."
        );

        // 2. config engine set, schedule weekly_day = Mon
        write(
            root.path(),
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local"}}}"#,
        );
        write(
            root.path(),
            "config/schedules.json",
            r#"{"weekly_day":"Mon"}"#,
        );
        assert_eq!(card(&ctx)["state"], "first");
        assert_eq!(card(&ctx)["line"], "your first week comes on monday.");

        // 3. invalid week file (version 2)
        write(
            root.path(),
            "reflections/weekly/20260809.json",
            r#"{"version":2,"days":[],"memories":[]}"#,
        );
        assert_eq!(card(&ctx)["state"], "unreadable");
        assert_eq!(card(&ctx)["line"], "the week of august 9 couldn't be read.");

        // 4. good week file (Sunday 20260809)
        write(
            root.path(),
            "reflections/weekly/20260809.json",
            &valid_week_json("20260809"),
        );
        assert_eq!(card(&ctx)["state"], "week");
        assert_eq!(card(&ctx)["url"], "/app/home/week/20260809");
        assert_eq!(card(&ctx)["title"], "week of august 9");
        assert_eq!(card(&ctx)["memory"], "Memory for sunday.");

        // 5. all memories left out
        let mut keys = BTreeSet::new();
        keys.insert("k0".to_string());
        write(
            root.path(),
            "health/week-left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );
        let card_all_out = card(&ctx);
        assert_eq!(card_all_out["state"], "week");
        assert!(card_all_out["memory"].is_null());
        assert_eq!(card_all_out["empty"], "nothing from this week is showing.");

        // 6. Pre-format .md only with engine set -> next week card
        let root2 = TempDir::new().unwrap();
        let ctx2 = HomeContext::with_zone(root2.path(), now, chrono_tz::Tz::UTC);
        write(
            root2.path(),
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local"}}}"#,
        );
        write(
            root2.path(),
            "config/schedules.json",
            r#"{"weekly_day":"Mon"}"#,
        );
        write(
            root2.path(),
            "reflections/weekly/20260810.md",
            "# pre-format",
        );
        let card_md = card(&ctx2);
        assert_eq!(card_md["state"], "first");
        assert_eq!(card_md["line"], "your next week comes on monday.");

        // 7. Pre-format .md only with no engine set -> next week engine line
        let root3 = TempDir::new().unwrap();
        let ctx3 = HomeContext::with_zone(root3.path(), now, chrono_tz::Tz::UTC);
        write(
            root3.path(),
            "reflections/weekly/20260810.md",
            "# pre-format",
        );
        let card_md_no_engine = card(&ctx3);
        assert_eq!(card_md_no_engine["state"], "first");
        assert_eq!(
            card_md_no_engine["line"],
            "your next week comes once processing is set up."
        );

        // 8. Newer .md does not hide older trusted .json
        write(
            root3.path(),
            "reflections/weekly/20260809.json",
            &valid_week_json("20260809"),
        );
        write(
            root3.path(),
            "reflections/weekly/20260816.md",
            "# newer pre-format",
        );
        let card_older_json = card(&ctx3);
        assert_eq!(card_older_json["state"], "week");
        assert_eq!(card_older_json["url"], "/app/home/week/20260809");

        // 9. Card walks days order, not memories order
        let root4 = TempDir::new().unwrap();
        let ctx4 = HomeContext::with_zone(root4.path(), now, chrono_tz::Tz::UTC);
        let days_order_json = r#"{
            "version": 1,
            "week": {"start":"20260308","end":"20260314"},
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m_first_day"},
                {"day":"20260309","state":"memory","memory_id":"m_second_day"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m_second_day",
                    "key": "k_second",
                    "day": "20260309",
                    "text": "Second day memory text.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260309/talents/morning_briefing"}
                },
                {
                    "id": "m_first_day",
                    "key": "k_first",
                    "day": "20260308",
                    "text": "First day memory text.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                }
            ]
        }"#;
        write(
            root4.path(),
            "reflections/weekly/20260308.json",
            days_order_json,
        );
        assert_eq!(card(&ctx4)["memory"], "First day memory text.");
    }

    #[test]
    fn stem_validation_protects_has_page_and_judge() {
        let root = TempDir::new().unwrap();
        for invalid in [
            "99999999",
            "..",
            "20260231",
            "not_a_day",
            "2026",
            "202608101",
        ] {
            assert!(!has_page(root.path(), invalid));
            assert_eq!(judge(root.path(), invalid), WeekJudgment::CouldntCheck);
        }
    }

    #[test]
    fn trust_cases_comprehensive() {
        let root = TempDir::new().unwrap();
        let sun = "20260308";
        // Missing non-Sunday stem is Absent when neither json nor md exists
        assert_eq!(judge(root.path(), "20260309"), WeekJudgment::Absent);

        // Present non-Sunday JSON is CouldntCheck
        write(
            root.path(),
            "reflections/weekly/20260309.json",
            &valid_week_json("20260309"),
        );
        assert_eq!(judge(root.path(), "20260309"), WeekJudgment::CouldntCheck);
        fs::remove_file(root.path().join("reflections/weekly/20260309.json")).unwrap();

        // Both json and md missing -> Absent
        assert_eq!(judge(root.path(), sun), WeekJudgment::Absent);

        // md only regular file -> CantShow
        write(
            root.path(),
            "reflections/weekly/20260308.md",
            "# Pre-format",
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CantShow);
        fs::remove_file(root.path().join("reflections/weekly/20260308.md")).unwrap();

        // md directory -> Absent
        fs::create_dir_all(root.path().join("reflections/weekly/20260308.md")).unwrap();
        assert_eq!(judge(root.path(), sun), WeekJudgment::Absent);
        fs::remove_dir(root.path().join("reflections/weekly/20260308.md")).unwrap();

        // md self-symlink (ELOOP) -> CouldntCheck
        symlink(
            root.path().join("reflections/weekly/20260308.md"),
            root.path().join("reflections/weekly/20260308.md"),
        )
        .unwrap();
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);
        fs::remove_file(root.path().join("reflections/weekly/20260308.md")).unwrap();

        // Bad JSON syntax -> CouldntCheck
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            "not json {",
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Missing version -> CouldntCheck
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            r#"{"days":[]}"#,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Version string "1" -> CantShow
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            r#"{"version":"1","days":[]}"#,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CantShow);

        // Version integer 2 -> CantShow
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            r#"{"version":2,"days":[]}"#,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CantShow);

        // Non-briefing source kind -> CantShow
        let non_briefing_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m0"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m0",
                    "key": "k0",
                    "day": "20260308",
                    "text": "Text.",
                    "source": {"kind":"external","uri":"sol://other"}
                }
            ]
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            non_briefing_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CantShow);

        // 6 days -> CouldntCheck
        let six_days_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"nothing_shared"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"}
            ],
            "memories": []
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            six_days_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Unknown state -> CouldntCheck
        let unknown_state_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"unknown_state"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": []
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            unknown_state_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Shuffled days -> CouldntCheck
        let shuffled_days_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260308","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": []
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            shuffled_days_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Memory day mismatch -> CouldntCheck
        let mismatch_day_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m0"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m0",
                    "key": "k0",
                    "day": "20260309",
                    "text": "Text.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260309/talents/morning_briefing"}
                }
            ]
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            mismatch_day_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Dangling memory ID -> CouldntCheck
        let dangling_id_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m_dangling"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": []
        }"#;
        write(
            root.path(),
            "reflections/weekly/20260308.json",
            dangling_id_json,
        );
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);

        // Duplicate memory ID in memories array -> CouldntCheck
        let dup_id_json = r#"{
            "version": 1,
            "days": [
                {"day":"20260308","state":"memory","memory_id":"m0"},
                {"day":"20260309","state":"nothing_shared"},
                {"day":"20260310","state":"nothing_shared"},
                {"day":"20260311","state":"nothing_shared"},
                {"day":"20260312","state":"nothing_shared"},
                {"day":"20260313","state":"nothing_shared"},
                {"day":"20260314","state":"nothing_shared"}
            ],
            "memories": [
                {
                    "id": "m0",
                    "key": "k0",
                    "day": "20260308",
                    "text": "Text.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                },
                {
                    "id": "m0",
                    "key": "k1",
                    "day": "20260308",
                    "text": "Text 2.",
                    "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
                }
            ]
        }"#;
        write(root.path(), "reflections/weekly/20260308.json", dup_id_json);
        assert_eq!(judge(root.path(), sun), WeekJudgment::CouldntCheck);
    }
}
