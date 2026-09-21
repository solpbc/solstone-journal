// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure briefing projections.

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use serde_json::{Value, json};

/// Daily artifacts belong to their analysis day; their briefing presents the
/// following local calendar day. This relation is independent of execution time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BriefingDates {
    pub analysis: NaiveDate,
    pub presentation: NaiveDate,
}

impl BriefingDates {
    pub fn for_analysis(analysis: NaiveDate) -> Option<Self> {
        Some(Self {
            analysis,
            presentation: analysis.succ_opt()?,
        })
    }

    pub fn for_presentation(presentation: NaiveDate) -> Option<Self> {
        Some(Self {
            analysis: presentation.pred_opt()?,
            presentation,
        })
    }
}

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
            "morning briefing: {meetings} {}, {needs_count} {} attention",
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
                    return format!("morning briefing: {line}");
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
    {
        let (key, left, right) = ("reading", "facet", "summary");
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
                                format!("- **{left}**: {right}")
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
    let your_day = briefing
        .get("your_day")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_object)
                .filter_map(|row| {
                    let text = row.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    let label = your_day_time_label(row);
                    (!label.is_empty() || !text.is_empty()).then(|| {
                        if !label.is_empty() && !text.is_empty() {
                            format!("- **{label}**: {text}")
                        } else if !label.is_empty() {
                            format!("- **{label}**")
                        } else {
                            format!("- {text}")
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !your_day.is_empty() {
        sections.insert("your_day".to_owned(), your_day.into());
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
                .filter_map(Value::as_object)
                .filter(|row| !your_day_time_label(row).is_empty())
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

fn field_text<'a>(row: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

/// A `your_day` item's clock label, mirroring `solstone-core-format`'s
/// `content::morning_briefing::time_label` (duplicated rather than shared --
/// the two crates render different surfaces and neither depends on the
/// other). `start == end` (or either one blank) collapses to a single point;
/// `end` alone with no `start` still reads as a point-in-time at `end`.
fn your_day_time_label(row: &serde_json::Map<String, Value>) -> String {
    let start = field_text(row, "start");
    let end = field_text(row, "end");
    match (start.is_empty(), end.is_empty()) {
        (true, true) => String::new(),
        (false, true) => start.to_owned(),
        (true, false) => end.to_owned(),
        (false, false) if start == end => start.to_owned(),
        (false, false) => format!("{start}–{end}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_day_renders_a_window_and_treats_end_only_as_a_point() {
        let sections = render_sections(&json!({"your_day":[
            {"start":"13:00","end":"13:30","text":"Team sync."},
            {"start":"","end":"14:00","text":"Odd shape."},
            {"start":"","end":"","text":"No fixed time."}
        ]}));
        assert_eq!(
            sections["your_day"],
            "- **13:00–13:30**: Team sync.\n- **14:00**: Odd shape.\n- No fixed time."
        );
    }

    #[test]
    fn meeting_count_counts_either_side_of_the_window_and_ignores_untimed_items() {
        let briefing = json!({"your_day":[
            {"start":"09:00","end":"09:00","text":"a"},
            {"start":"","end":"10:00","text":"b"},
            {"start":"","end":"","text":"c"}
        ]});
        assert_eq!(meeting_count(&briefing), 2);
    }

    #[test]
    fn summary_reads_meeting_count_off_the_new_start_end_shape() {
        let briefing = json!({"your_day":[{"start":"09:00","end":"09:30","text":"a"}]});
        let sections = render_sections(&briefing);
        assert_eq!(
            summary(Some(&briefing), &sections, 0),
            "morning briefing: 1 meeting, 0 items need attention"
        );
    }
}
