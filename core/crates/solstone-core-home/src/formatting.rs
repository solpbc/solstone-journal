// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure presentation helpers for the home surface.

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};

pub fn format_date(day: &str) -> String {
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return day.to_owned();
    };
    let suffix = match date.day() {
        11..=13 => "th",
        value => match value % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    };
    format!(
        "{} {} {}{}",
        date.format("%A"),
        date.format("%B"),
        date.day(),
        suffix
    )
}

pub fn relative_time(seconds: f64) -> String {
    let seconds = if seconds.is_finite() && seconds >= 0.0 {
        seconds as i64
    } else {
        0
    };
    if seconds < 60 {
        return plural(seconds, "second");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return plural(minutes, "minute");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return plural(hours, "hour");
    }
    let days = hours / 24;
    if days < 7 {
        return plural(days, "day");
    }
    if days < 28 {
        return plural(days / 7, "week");
    }
    if days < 60 {
        return "1 month".to_owned();
    }
    plural(days / 30, "month")
}

pub fn format_duration(minutes: f64) -> String {
    let rounded = round_half_even(minutes) as i64;
    if rounded < 60 {
        return format!("{rounded} min");
    }
    let hours = (minutes / 60.0 * 10.0).round() / 10.0;
    if hours.fract() == 0.0 {
        return format!(
            "{} hour{}",
            hours as i64,
            if hours == 1.0 { "" } else { "s" }
        );
    }
    format!("{hours:.1} hours")
}

pub fn format_hour_label(start: i64, end: i64) -> String {
    let meridiem = |hour: i64| if hour.rem_euclid(24) < 12 { "am" } else { "pm" };
    let render = |hour: i64, suffix: bool| {
        let hour = hour.rem_euclid(24);
        let display = hour % 12;
        let display = if display == 0 { 12 } else { display };
        if suffix {
            format!("{display}{}", meridiem(hour))
        } else {
            display.to_string()
        }
    };
    format!(
        "{}-{}",
        render(start, meridiem(start) != meridiem(end)),
        render(end, true)
    )
}

pub fn join_phrases(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.to_owned(),
        [left, right] => format!("{left} and {right}"),
        _ => format!(
            "{}, and {}",
            parts[..parts.len() - 1].join(", "),
            parts.last().unwrap()
        ),
    }
}

pub fn normalize_activity_title(record: &Value) -> String {
    record
        .get("description")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            record
                .get("activity")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| title_case(&value.replace('_', " ")))
        })
        .unwrap_or_else(|| "untitled activity".to_owned())
}

pub fn format_activity_label(activity: &Value) -> String {
    let title = activity
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("untitled activity")
        .trim();
    let duration = activity
        .get("duration_minutes")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let facet = activity
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    // `title` can be a short phrase ("wrote the spec") or a full sentence
    // pulled from a talent-generated description ("Discussion involves...").
    // Putting it last, after a colon, reads naturally either way; embedding
    // it mid-sentence produced run-ons when title was already a sentence.
    let terminator = if title.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    format!(
        "I spent {} taking notes in {facet}: {title}{terminator}",
        format_duration(duration)
    )
}

pub fn format_newsletter_summary(successful: i64, attempted: i64) -> String {
    if attempted == 0 {
        return "I didn't produce any facet newsletters.".to_owned();
    }
    if attempted > successful {
        return format!(
            "I wrote {successful} of {attempted} newsletter{}.",
            if attempted == 1 { "" } else { "s" }
        );
    }
    format!(
        "I wrote {successful} newsletter{}.",
        if successful == 1 { "" } else { "s" }
    )
}

pub fn format_processing_summary(
    mode: &str,
    successful: i64,
    attempted: i64,
    briefing_valid: bool,
) -> String {
    if mode == "degraded" {
        if attempted == 0 && successful == 0 {
            return "I didn't produce any facet newsletters, and some overnight processing didn't finish.".to_owned();
        }
        if attempted > successful {
            return format!(
                "I wrote {successful} of {attempted} newsletters, but some overnight processing didn't finish."
            );
        }
        return format!(
            "I wrote {successful} newsletter{}, but some overnight processing didn't finish.",
            if successful == 1 { "" } else { "s" }
        );
    }
    let mut actions = Vec::new();
    if successful > 0 {
        actions.push(format!(
            "wrote {successful} newsletter{}",
            if successful == 1 { "" } else { "s" }
        ));
    }
    if briefing_valid {
        actions.push("prepared your morning briefing".to_owned());
    }
    if actions.is_empty() {
        return format_newsletter_summary(successful, attempted);
    }
    format!("I {}.", join_phrases(&actions))
}

pub fn top_heatmap_hours(stats: &Value) -> Vec<i64> {
    let Some(hours) = stats
        .pointer("/heatmap_data/hours")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut rows = hours
        .iter()
        .filter_map(|(hour, amount)| Some((hour.parse::<i64>().ok()?, amount.as_f64()?)))
        .filter(|(_, amount)| *amount > 0.0)
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    rows.into_iter().take(3).map(|(hour, _)| hour).collect()
}

pub fn format_heatmap_summary(stats: &Value) -> Option<String> {
    let mut hours = top_heatmap_hours(stats);
    hours.sort_unstable();
    let first = *hours.first()?;
    let mut ranges = Vec::new();
    let mut start = first;
    let mut end = first + 1;
    for hour in hours.into_iter().skip(1) {
        if hour == end {
            end += 1;
        } else {
            ranges.push(format_hour_label(start, end));
            start = hour;
            end = hour + 1;
        }
    }
    ranges.push(format_hour_label(start, end));
    Some(format!(
        "your busiest stretches were {}.",
        ranges.join(" · ")
    ))
}

/// Compose the "what didn't finish" links for yesterday's processing.
///
/// `overnight_passed` says whether the local day has actually reached the far
/// side of the overnight window. The overnight review and the morning briefing
/// are produced inside it, so before it has passed neither can be reported as
/// unfinished — the work has not been given its chance yet.
pub fn format_gap_links(
    pipeline: &Value,
    briefing_valid: bool,
    yesterday: &str,
    today: &str,
    overnight_passed: bool,
) -> Vec<Value> {
    let anomalies = pipeline
        .get("anomalies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_daily = anomalies
        .iter()
        .any(|row| row.get("kind").and_then(Value::as_str) == Some("daily_agents_missing"));
    let has_activity = anomalies
        .iter()
        .any(|row| row.get("kind").and_then(Value::as_str) == Some("activity_agents_missing"));
    let failures = anomalies
        .iter()
        .filter(|row| row.get("kind").and_then(Value::as_str) == Some("talent_failure"))
        .collect::<Vec<_>>();
    let mut links = Vec::new();
    if has_daily && overnight_passed {
        links.push(json!({"text":"the overnight review didn't finish.","href":format!("/app/thinking/#runs/{yesterday}")}));
    }
    if has_activity {
        links.push(json!({"text":"I didn't finish writing all of yesterday's notes.","href":format!("/app/thinking/#runs/{yesterday}")}));
    }
    let mut groups = std::collections::BTreeMap::<String, Vec<&Value>>::new();
    for failure in &failures {
        let name = failure
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !name.is_empty() {
            groups.entry(name.to_owned()).or_default().push(*failure);
        }
    }
    let has_named_failures = !groups.is_empty();
    for (name, mut rows) in groups {
        rows.sort_by(|left, right| {
            left["mode"]
                .as_str()
                .cmp(&right["mode"].as_str())
                .then_with(|| left["use_id"].as_str().cmp(&right["use_id"].as_str()))
        });
        let count = rows.len();
        // Talent names are internal identifiers and can be namespaced with a
        // colon (e.g. "entities:detection"); sanitize both separators so the
        // raw identifier never leaks into owner-facing copy.
        let label = singularize_talent_label(&name.replace(['_', ':'], " "));
        let lost = rows
            .iter()
            .all(|row| row.get("state").and_then(Value::as_str) == Some("request_lost"));
        let text = if count == 1 {
            format!(
                "The {label} run {}.",
                if lost {
                    "couldn't start"
                } else {
                    "didn't finish"
                }
            )
        } else {
            format!(
                "{count} {label} runs {}.",
                if lost {
                    "couldn't start"
                } else {
                    "didn't finish"
                }
            )
        };
        let use_id = rows[0]
            .get("use_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let href = if count == 1 && !use_id.is_empty() {
            format!("/app/thinking/#runs/{yesterday}/{name}/{use_id}")
        } else {
            format!("/app/thinking/#runs/{yesterday}/{name}")
        };
        links.push(json!({"text":text,"href":href}));
    }
    if pipeline
        .pointer("/talents/failed_list_truncated")
        .and_then(Value::as_bool)
        == Some(true)
    {
        let more = pipeline
            .pointer("/talents/outstanding_failed")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            - failures.len() as i64;
        if more > 0 {
            links.push(json!({"text":format!("…and {more} more didn't finish."),"href":format!("/app/thinking/#runs/{yesterday}")}));
        }
    }
    if !failures.is_empty() && !has_daily && !has_activity && !has_named_failures {
        links.push(json!({"text":"Some of my overnight work didn't finish.","href":format!("/app/thinking/#runs/{yesterday}")}));
    }
    if !briefing_valid && overnight_passed {
        links.push(json!({"text":"your morning briefing wasn't prepared overnight.","href":format!("/app/thinking/#runs/{today}/morning_briefing")}));
    }
    links
}

/// Total count of talent runs that failed, honest about truncation: the
/// `anomalies` array `format_gap_links` groups from may itself be a capped
/// view, with the true total tracked separately in `outstanding_failed`.
pub fn count_failed_runs(pipeline: &Value) -> i64 {
    let visible = pipeline
        .get("anomalies")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("kind").and_then(Value::as_str) == Some("talent_failure"))
                .count() as i64
        })
        .unwrap_or(0);
    if pipeline
        .pointer("/talents/failed_list_truncated")
        .and_then(Value::as_bool)
        == Some(true)
    {
        pipeline
            .pointer("/talents/outstanding_failed")
            .and_then(Value::as_i64)
            .filter(|total| *total > visible)
            .unwrap_or(visible)
    } else {
        visible
    }
}

/// A talent's internal identifier is occasionally already plural (e.g. the
/// "documents" talent); the sentence template always appends "run"/"runs" on
/// top, so an already-plural label produces "documents runs". Known cases are
/// singularized for display only — the raw identifier (used for hrefs) is
/// untouched.
fn singularize_talent_label(label: &str) -> String {
    match label {
        "documents" => "document".to_owned(),
        other => other.to_owned(),
    }
}

fn plural(value: i64, unit: &str) -> String {
    format!("{value} {unit}{}", if value == 1 { "" } else { "s" })
}
fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    let fraction = value - floor;
    if (fraction - 0.5).abs() < f64::EPSILON {
        if (floor as i64).rem_euclid(2) == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_gap_links_cap_and_sort_the_first_anchor() {
        let failures = (0..20)
            .map(|index| {
                json!({
                    "kind":"talent_failure",
                    "name":format!("task_{index}"),
                    "mode":"daily",
                    "use_id":format!("{index:02}"),
                    "state":"failed",
                })
            })
            .collect::<Vec<_>>();
        let links = format_gap_links(
            &json!({"anomalies":failures,"talents":{"failed_list_truncated":true,"outstanding_failed":21}}),
            true,
            "20260813",
            "20260814",
            true,
        );
        assert_eq!(links.last().unwrap()["text"], "…and 1 more didn't finish.");

        let links = format_gap_links(
            &json!({"anomalies":[
                {"kind":"talent_failure","name":"daily_summary","mode":"daily","use_id":"z","state":"failed"},
                {"kind":"talent_failure","name":"daily_summary","mode":"activity","use_id":"a","state":"failed"}
            ]}),
            true,
            "20260813",
            "20260814",
            true,
        );
        assert_eq!(links[0]["text"], "2 daily summary runs didn't finish.");
        assert_eq!(
            links[0]["href"],
            "/app/thinking/#runs/20260813/daily_summary"
        );

        let links = format_gap_links(
            &json!({"anomalies":[
                {"kind":"talent_failure","name":"daily_summary","mode":"daily","use_id":"z","state":"failed"},
                {"kind":"talent_failure","name":"other","mode":"daily","use_id":"x","state":"failed"}
            ]}),
            true,
            "20260813",
            "20260814",
            true,
        );
        assert_eq!(
            links[0]["href"],
            "/app/thinking/#runs/20260813/daily_summary/z"
        );
    }

    #[test]
    fn failure_gap_link_label_strips_colon_namespacing() {
        let links = format_gap_links(
            &json!({"anomalies":[
                {"kind":"talent_failure","name":"entities:detection","mode":"daily","use_id":"a","state":"failed"}
            ]}),
            true,
            "20260813",
            "20260814",
            true,
        );
        assert_eq!(
            links[0]["text"],
            "The entities detection run didn't finish."
        );
        assert_eq!(
            links[0]["href"],
            "/app/thinking/#runs/20260813/entities:detection/a"
        );
    }

    #[test]
    fn failure_gap_link_label_singularizes_a_plural_talent_name() {
        let links = format_gap_links(
            &json!({"anomalies":[
                {"kind":"talent_failure","name":"documents","mode":"daily","use_id":"a","state":"failed"},
                {"kind":"talent_failure","name":"documents","mode":"daily","use_id":"b","state":"failed"}
            ]}),
            true,
            "20260813",
            "20260814",
            true,
        );
        assert_eq!(links[0]["text"], "2 document runs didn't finish.");
        assert_eq!(links[0]["href"], "/app/thinking/#runs/20260813/documents");
    }
}
