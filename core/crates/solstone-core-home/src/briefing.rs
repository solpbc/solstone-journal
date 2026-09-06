// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure briefing projections.

use chrono::{DateTime, Timelike, Utc};
use serde_json::{Value, json};

pub fn compute_phase(segment_count: i64, hour: u32, exists: bool) -> &'static str {
    if hour >= 20 {
        "eod"
    } else if !exists && hour < 10 {
        "pending"
    } else if exists && (segment_count == 0 || hour < 10) {
        "morning"
    } else if exists && segment_count > 0 {
        "active"
    } else if !exists {
        // A briefing that was never prepared between the morning end and the
        // evening: the card says so rather than disappearing.
        "missing"
    } else {
        "eod"
    }
}

pub fn lateness_state(now: DateTime<Utc>, phase: &str) -> Value {
    let late = phase == "missing" || (phase == "pending" && now.hour() > 12);
    json!({"late":late,"late_hours":if late { (i64::from(now.hour()) - 10).max(0) } else { 0 }})
}

pub fn summary(briefing: Option<&Value>, sections: &Value, needs_count: i64) -> String {
    let meetings = briefing.map(meeting_count).unwrap_or(0);
    if meetings > 0 || needs_count > 0 {
        return format!(
            "morning briefing — {meetings} {}, {needs_count} {} attention",
            if meetings == 1 { "meeting" } else { "meetings" },
            if needs_count == 1 {
                "item needs"
            } else {
                "items need"
            }
        );
    }
    if let Some(sections) = sections.as_object() {
        for content in sections.values().filter_map(Value::as_str) {
            for line in content.lines() {
                let line = line.trim().trim_start_matches("- ").trim();
                if !line.is_empty() {
                    let line = if line.len() > 58 {
                        format!("{}...", line[..55].trim_end())
                    } else {
                        line.to_owned()
                    };
                    return format!("morning briefing — {line}");
                }
            }
        }
    }
    "morning briefing".to_owned()
}

pub fn render_sections(briefing: &Value) -> Value {
    let mut sections = serde_json::Map::new();
    for key in ["yesterday", "forward_look"] {
        let rows = briefing
            .get(key)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(value_text)
                    .map(|text| text.trim().to_owned())
                    .filter(|text| !text.is_empty())
                    .map(|text| format!("- {text}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !rows.is_empty() {
            sections.insert(key.to_owned(), rows.into());
        }
    }
    for (key, left, right) in [
        ("your_day", "time", "text"),
        ("reading", "facet", "summary"),
    ] {
        let rows = briefing
            .get(key)
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_object)
                    .filter_map(|row| {
                        let left = row.get(left).and_then(Value::as_str).unwrap_or("").trim();
                        let right = row.get(right).and_then(Value::as_str).unwrap_or("").trim();
                        (!left.is_empty() || !right.is_empty()).then(|| {
                            if !left.is_empty() && !right.is_empty() {
                                format!("- **{left}** — {right}")
                            } else if !left.is_empty() {
                                format!("- **{left}**")
                            } else {
                                format!("- {right}")
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !rows.is_empty() {
            sections.insert(key.to_owned(), rows.into());
        }
    }
    let needs = needs_items(briefing)
        .iter()
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
        sections.insert("needs_attention".to_owned(), needs.into());
    }
    Value::Object(sections)
}

pub fn needs_items(briefing: &Value) -> Vec<Value> {
    briefing
        .get("needs_attention")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter(|row| row.is_object()).cloned().collect())
        .unwrap_or_default()
}
pub fn meeting_count(briefing: &Value) -> i64 {
    briefing
        .get("your_day")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| {
                    row.get("time")
                        .and_then(Value::as_str)
                        .is_some_and(|time| !time.trim().is_empty())
                })
                .count() as i64
        })
        .unwrap_or(0)
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
