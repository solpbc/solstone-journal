// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

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

#[derive(Deserialize)]
struct RawWeekFile {
    version: u32,
    week: Option<RawWeekRange>,
    days: Option<Vec<RawDay>>,
    memories: Option<Vec<RawMemory>>,
}

#[derive(Deserialize)]
struct RawWeekRange {
    start: Option<String>,
    end: Option<String>,
}

#[derive(Deserialize)]
struct RawDay {
    day: Option<String>,
    state: Option<String>,
    memory_id: Option<String>,
}

#[derive(Deserialize)]
struct RawMemory {
    id: Option<String>,
    key: Option<String>,
    day: Option<String>,
    text: Option<String>,
    source: Option<RawSource>,
}

#[derive(Deserialize)]
struct RawSource {
    kind: Option<String>,
    uri: Option<String>,
    briefing_day: Option<String>,
    #[serde(default)]
    refs: Vec<String>,
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

pub fn week_day(segment: &str) -> Option<NaiveDate> {
    if segment.len() != 8 || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    NaiveDate::parse_from_str(segment, "%Y%m%d").ok()
}

pub fn judge(journal: &Path, day: &str) -> WeekJudgment {
    if week_day(day).is_none() {
        return WeekJudgment::CouldntCheck;
    }
    let json_path = journal.join(format!("reflections/weekly/{day}.json"));
    let content = match fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let md_path = journal.join(format!("reflections/weekly/{day}.md"));
            return if md_path.is_file() {
                WeekJudgment::CantShow
            } else {
                WeekJudgment::Absent
            };
        }
        Err(_) => return WeekJudgment::CouldntCheck,
    };

    let raw: RawWeekFile = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(_) => return WeekJudgment::CouldntCheck,
    };

    if raw.version != 1 {
        return WeekJudgment::CantShow;
    }

    let memories_raw = raw.memories.unwrap_or_default();
    let mut trusted_memories = Vec::with_capacity(memories_raw.len());
    let mut memory_ids = BTreeSet::new();

    for m in memories_raw {
        let (Some(id), Some(key), Some(day), Some(text), Some(source)) =
            (m.id, m.key, m.day, m.text, m.source)
        else {
            return WeekJudgment::CouldntCheck;
        };

        let (Some(kind), Some(uri)) = (source.kind, source.uri) else {
            return WeekJudgment::CouldntCheck;
        };

        if kind != "briefing" {
            return WeekJudgment::CantShow;
        }

        memory_ids.insert(id.clone());
        trusted_memories.push(TrustedMemory {
            id,
            key,
            day,
            text,
            source: TrustedSource {
                kind,
                uri,
                briefing_day: source.briefing_day,
                refs: source.refs,
            },
        });
    }

    let Some(days_raw) = raw.days else {
        return WeekJudgment::CouldntCheck;
    };

    if days_raw.len() != 7 {
        return WeekJudgment::CouldntCheck;
    }

    let mut trusted_days = Vec::with_capacity(7);
    for d in days_raw {
        let (Some(day_str), Some(state)) = (d.day, d.state) else {
            return WeekJudgment::CouldntCheck;
        };

        if state == "memory" {
            let Some(mem_id) = &d.memory_id else {
                return WeekJudgment::CouldntCheck;
            };
            if !memory_ids.contains(mem_id) {
                return WeekJudgment::CouldntCheck;
            }
        }

        trusted_days.push(TrustedDay {
            day: day_str,
            state,
            memory_id: d.memory_id,
        });
    }

    let start_day = raw
        .week
        .as_ref()
        .and_then(|w| w.start.clone())
        .unwrap_or_else(|| day.to_string());
    let end_day = raw
        .week
        .as_ref()
        .and_then(|w| w.end.clone())
        .unwrap_or_else(|| day.to_string());

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
    let path = journal.join("reflections/weekly/left-out.json");
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
    let read_res = fs::read_dir(&weekly_dir);

    let entries = match read_res {
        Ok(e) => e,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return first_card(context);
        }
        Err(_) => {
            return json!({
                "state": "unchecked",
                "title": Value::Null,
                "url": Value::Null,
                "memory": Value::Null,
                "empty": Value::Null,
                "line": "your week couldn't be checked.",
            });
        }
    };

    let mut candidate_stems = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|stem| week_day(stem).is_some())
        {
            candidate_stems.push(stem.to_string());
        }
    }

    if candidate_stems.is_empty() {
        return first_card(context);
    }

    candidate_stems.sort();
    let newest = candidate_stems.pop().unwrap();

    let current_year = context.local_date().year();
    let stem_date = week_day(&newest).unwrap();

    match judge(context.journal_root(), &newest) {
        WeekJudgment::Page(trusted) => {
            let title = format_week_title(stem_date, current_year);
            let url = format!("/app/home/week/{newest}");

            let left_out = read_left_out(context.journal_root());
            let (memory, empty) = match left_out {
                LeftOut::Unreadable => (Value::Null, Value::Null),
                LeftOut::Keys(keys) => {
                    let first_shown = trusted.memories.iter().find(|m| !keys.contains(&m.key));
                    if let Some(m) = first_shown {
                        (Value::String(m.text.clone()), Value::Null)
                    } else {
                        (
                            Value::Null,
                            Value::String("nothing from this week is showing.".to_string()),
                        )
                    }
                }
                LeftOut::None => {
                    if let Some(m) = trusted.memories.first() {
                        (Value::String(m.text.clone()), Value::Null)
                    } else {
                        (
                            Value::Null,
                            Value::String("nothing from this week is showing.".to_string()),
                        )
                    }
                }
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
        WeekJudgment::Absent => first_card(context),
    }
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
        // Week A (20250914), Week B (20260301 md only), Week C (20260308 json)
        write(
            root.path(),
            "reflections/weekly/20250914.json",
            fixture_json,
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
            "reflections/weekly/left-out.json",
            &String::from_utf8(left_out_bytes(&keys)).unwrap(),
        );

        let model = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(model["intro"], "your week, a memory from each of 2 days.");
        assert_eq!(model["rows"][0]["left_out"], true);
        assert!(model["rows"][0].get("text").is_none());
        assert_eq!(model["cells"][0]["state"], "not_on_page");
        assert_eq!(model["from_line"], "from monday and tuesday.");

        // Unreadable left-out file
        write(
            root.path(),
            "reflections/weekly/left-out.json",
            "garbage bytes",
        );
        let model_unreadable = page_model(root.path(), "20260308", 2026, &|_| false).unwrap();
        assert_eq!(
            model_unreadable["left_out_notice"],
            "your left-out memories couldn't be checked, so everything is showing."
        );
        assert_eq!(model_unreadable["rows"].as_array().unwrap().len(), 3);
        assert_eq!(model_unreadable["rows"][0]["left_out"], false);
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
            "reflections/weekly/20260810.json",
            r#"{"version":2,"days":[],"memories":[]}"#,
        );
        assert_eq!(card(&ctx)["state"], "unreadable");
        assert_eq!(
            card(&ctx)["line"],
            "the week of august 10 couldn't be read."
        );

        // 4. good week file
        let fixture_json =
            include_str!("../../../../tests/fixtures/journal/reflections/weekly/20260308.json");
        write(
            root.path(),
            "reflections/weekly/20260810.json",
            fixture_json,
        );
        assert_eq!(card(&ctx)["state"], "week");
        assert_eq!(card(&ctx)["url"], "/app/home/week/20260810");
        assert_eq!(card(&ctx)["title"], "week of august 10");
        assert!(!card(&ctx)["memory"].is_null());
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
}
