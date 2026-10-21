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

pub fn summary(sections: &Value, needs_count: i64) -> String {
    if needs_count > 0 {
        return format!(
            "morning briefing: {needs_count} {} attention",
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
                        let cut = line.floor_char_boundary(55);
                        format!("{}...", line[..cut].trim_end())
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
    let yesterday = briefing
        .get("yesterday")
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
    if !yesterday.is_empty() {
        sections.insert("yesterday".to_owned(), yesterday.into());
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

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_leads_with_the_attention_count_when_there_is_one() {
        let sections = json!({"yesterday": "- shipped"});
        let with_needs = summary(&sections, 3);
        assert!(with_needs.contains('3'));
        assert!(!with_needs.contains("shipped"));
        assert!(summary(&sections, 0).contains("shipped"));
    }

    #[test]
    fn summary_truncates_at_utf8_char_boundary_without_tearing() {
        let prefix = "a".repeat(54);
        let raw_line = format!("{prefix}—xy");
        assert!(raw_line.len() > 58);
        let sections = json!({"yesterday": format!("- {raw_line}")});
        let result = summary(&sections, 0);
        let prefix_part = result
            .strip_prefix("morning briefing: ")
            .and_then(|s| s.strip_suffix("..."))
            .expect("prefix and suffix present");
        assert!(prefix_part.len() <= 55);
        assert!(raw_line.starts_with(prefix_part));
        assert!(!prefix_part.ends_with('—'));
    }

    #[test]
    fn render_sections_omits_retired_sections_even_when_present() {
        let briefing = json!({
            "yesterday": ["shipped"],
            "your_day": [{"start": "09:00", "end": "09:30", "text": "standup"}],
            "forward_look": ["plan the week"],
            "reading": [{"facet": "work", "summary": "important items"}]
        });
        let sections = render_sections(&briefing);
        assert_eq!(sections, json!({"yesterday": "- shipped"}));
    }
}
