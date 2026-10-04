// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Request-time prompt variables preserved from the Python talent runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{Days, NaiveDate};
use serde_json::{Map, Value};
use solstone_core_format::segment::segment_start_and_end_seconds;
use solstone_core_system_health::find_segment_dir;

pub(crate) fn build(
    journal: &Path,
    config: &Map<String, Value>,
    binding: Option<&Map<String, Value>>,
) -> BTreeMap<String, String> {
    let mut context = BTreeMap::new();
    let Some(day) = config
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty())
    else {
        return context;
    };

    context.insert("day".to_owned(), format_day(day));
    context.insert("day_YYYYMMDD".to_owned(), day.to_owned());
    // The weekly reflection receives a week-start anchor.
    if config.get("schedule").and_then(Value::as_str) == Some("weekly")
        && let Ok(reference_day) = NaiveDate::parse_from_str(day, "%Y%m%d")
        && let Some(bound) = reference_day.checked_add_days(Days::new(6))
    {
        context.insert(
            "week_end_YYYYMMDD".to_owned(),
            bound.format("%Y%m%d").to_string(),
        );
        if let Some(days) = calendar_days(reference_day, 7) {
            context.insert("week_days_YYYYMMDD".to_owned(), days);
        }
    }

    let configured_stream = config
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| !stream.is_empty())
        .map(str::to_owned);
    let environment_stream = std::env::var("SOL_STREAM")
        .ok()
        .filter(|stream| !stream.is_empty());
    let stream = configured_stream.or(environment_stream);
    context.insert(
        "stream".to_owned(),
        stream.clone().unwrap_or_else(|| "archon".to_owned()),
    );
    let (source, kind) = match binding {
        Some(map) => (
            map.get("source").and_then(Value::as_str),
            map.get("kind").and_then(Value::as_str),
        ),
        None => (None, None),
    };
    context.insert(
        "content_description".to_owned(),
        stream_content_description(stream.as_deref(), source, kind),
    );
    context.insert(
        "import_guidance".to_owned(),
        stream_import_guidance(stream.as_deref(), source, kind),
    );

    if let Some(segment) = config
        .get("segment")
        .and_then(Value::as_str)
        .filter(|segment| !segment.is_empty())
    {
        if let Some((start, end)) = formatted_segment_times(segment) {
            context.insert("segment".to_owned(), segment.to_owned());
            context.insert("segment_start".to_owned(), start);
            context.insert("segment_end".to_owned(), end);
        }
    } else if let Some(span) = string_array(config.get("span")) {
        let bounds = span
            .iter()
            .filter_map(|segment| segment_bounds(segment))
            .collect::<Vec<_>>();
        if let (Some(start), Some(end)) = (
            bounds.iter().map(|(start, _)| *start).min(),
            bounds.iter().map(|(_, end)| *end).max(),
        ) {
            context.insert("segment_start".to_owned(), format_time(start));
            context.insert("segment_end".to_owned(), format_time(end));
        }
    }

    if let Some(activity) = config
        .get("activity")
        .and_then(Value::as_object)
        .filter(|activity| !activity.is_empty())
    {
        let segments = string_array(activity.get("segments")).unwrap_or_default();
        let entities = string_array(activity.get("active_entities")).unwrap_or_default();
        context.insert("activity_id".to_owned(), python_string(activity.get("id")));
        context.insert(
            "activity_type".to_owned(),
            python_string(activity.get("activity")),
        );
        context.insert(
            "activity_description".to_owned(),
            python_string(activity.get("description")),
        );
        if let Some(level) = activity.get("level_avg").filter(|value| value.is_number()) {
            context.insert("activity_level".to_owned(), python_value_string(level));
        }
        context.insert("activity_entities".to_owned(), entities.join(", "));
        context.insert("activity_segments".to_owned(), segments.join(", "));
        context.insert(
            "activity_duration".to_owned(),
            estimate_duration_minutes(&segments).to_string(),
        );
    }

    if let Some(facet) = config
        .get("facet")
        .and_then(Value::as_str)
        .filter(|facet| !facet.is_empty())
    {
        context.insert("facet".to_owned(), facet.to_owned());
        context.insert(
            "activity_md_dir".to_owned(),
            format!("{}/facets/{facet}/activities/{day}/", journal.display()),
        );
    }

    if let (Some(activity), Some(span), Some(facet)) = (
        config
            .get("activity")
            .and_then(Value::as_object)
            .filter(|activity| !activity.is_empty()),
        string_array(config.get("span")).filter(|span| !span.is_empty()),
        config
            .get("facet")
            .and_then(Value::as_str)
            .filter(|facet| !facet.is_empty()),
    ) {
        context.insert(
            "activity_context".to_owned(),
            activity_context(
                journal,
                day,
                facet,
                activity,
                &span,
                activity
                    .get("stream")
                    .and_then(Value::as_str)
                    .or(stream.as_deref()),
            ),
        );
    }

    context
}

fn format_day(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|date| date.format("%A, %B %d, %Y").to_string())
        .unwrap_or_else(|_| day.to_owned())
}

fn segment_bounds(segment: &str) -> Option<(u64, u64)> {
    let (start, end) = segment_start_and_end_seconds(segment)?;
    let start =
        u64::from(start.hour) * 3_600 + u64::from(start.minute) * 60 + u64::from(start.second);
    Some((start, end))
}

fn formatted_segment_times(segment: &str) -> Option<(String, String)> {
    let (start, end) = segment_bounds(segment)?;
    Some((format_time(start), format_time(end)))
}

fn format_time(seconds: u64) -> String {
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let (hour, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{hour}:{minute:02} {meridiem}")
}

fn estimate_duration_minutes(segments: &[String]) -> u64 {
    let seconds = segments
        .iter()
        .filter_map(|segment| segment_bounds(segment))
        .map(|(start, end)| end.saturating_sub(start))
        .sum::<u64>();
    (seconds / 60).max(1)
}

fn activity_context(
    journal: &Path,
    day: &str,
    facet: &str,
    activity: &Map<String, Value>,
    span: &[String],
    stream: Option<&str>,
) -> String {
    let activity_type = activity
        .get("activity")
        .map(python_value_string)
        .unwrap_or_else(|| "unknown".to_owned());
    let engagement_line = activity
        .get("level_avg")
        .and_then(|value| value.as_f64().map(|level| (value, level)))
        .map(|(value, level)| {
            let label = if level >= 0.75 {
                "high"
            } else if level >= 0.4 {
                "medium"
            } else {
                "low"
            };
            format!(
                "- **Last engaged segment level:** {} ({label})\n",
                python_value_string(value)
            )
        })
        .unwrap_or_default();
    let segments = string_array(activity.get("segments")).unwrap_or_default();
    let entities = string_array(activity.get("active_entities")).unwrap_or_default();
    let entities = if entities.is_empty() {
        "none detected".to_owned()
    } else {
        entities.join(", ")
    };
    let mut parts = vec![format!(
        "## Activity Context\n- **Type:** {activity_type}\n- **Description:** {}\n{engagement_line}- **Duration:** ~{} minutes ({} segments)\n- **Active Entities:** {entities}",
        python_string(activity.get("description")),
        estimate_duration_minutes(&segments),
        segments.len(),
    )];

    let state_lines = span
        .iter()
        .filter_map(|segment| {
            let entry = load_segment_facet_classification(journal, day, segment, facet, stream)?;
            let time_label = formatted_segment_times(segment)
                .map(|(start, end)| format!(" ({start} - {end})"))
                .unwrap_or_default();
            Some(format!(
                "### {segment}{time_label}\n{facet} [{}]: {}",
                python_string(entry.get("level")),
                python_string(entry.get("activity")),
            ))
        })
        .collect::<Vec<_>>();
    // Only point the talent at the per-segment section when at least one
    // segment in the span has a facet classification to show.
    let guidance = if state_lines.is_empty() {
        "Use the Activity Context above to identify which content relates to this activity, and ignore unrelated content."
    } else {
        parts.push(format!(
            "## Activity State Per Segment\n\n{}",
            state_lines.join("\n\n")
        ));
        "Use the Activity State Per Segment section above to identify which content relates to this activity, and ignore unrelated content."
    };

    parts.push(format!(
        "## Analysis Focus\nYou are analyzing ONLY the **{activity_type}** activity within the **{facet}** facet. The transcript segments may contain content from other concurrent activities (e.g., background meetings, messaging). {guidance} Your analysis should only cover what happened within this specific activity."
    ));
    parts.join("\n\n")
}

/// The segment's Sense facet classification for `facet`: what was done for
/// that facet in this segment and how central it was (`activity`, `level`).
fn load_segment_facet_classification(
    journal: &Path,
    day: &str,
    segment: &str,
    facet: &str,
    stream: Option<&str>,
) -> Option<Map<String, Value>> {
    let segment = find_segment_dir(journal, day, segment, stream)?;
    let bytes = fs::read(segment.join("talents").join("facets.json")).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|entry| entry.get("facet").and_then(Value::as_str) == Some(facet))
        .filter(|entry| {
            entry
                .get("activity")
                .and_then(Value::as_str)
                .is_some_and(|activity| !activity.trim().is_empty())
        })
        .cloned()
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value?.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

fn python_string(value: Option<&Value>) -> String {
    value.map(python_value_string).unwrap_or_default()
}

fn python_value_string(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn is_browser_route(stream: Option<&str>, source: Option<&str>, kind: Option<&str>) -> bool {
    if source == Some("browser") || kind == Some("browser") {
        return true;
    }
    if source.is_some() {
        return false;
    }
    if kind.is_none() && stream.is_some_and(|s| s.ends_with(".browser")) {
        return true;
    }
    false
}

fn stream_content_description(
    stream: Option<&str>,
    source: Option<&str>,
    kind: Option<&str>,
) -> String {
    if is_browser_route(stream, source, kind) {
        return "semantic page text and change updates from browser web apps such as Gmail or Slack"
            .to_owned();
    }
    if let Some(stream) = stream
        && stream.starts_with("import.")
    {
        match stream {
            "import.chatgpt" => return "an imported ChatGPT conversation".to_owned(),
            "import.claude" => return "an imported Claude conversation".to_owned(),
            "import.gemini" => return "an imported Gemini conversation".to_owned(),
            "import.ics" => return "an imported calendar event".to_owned(),
            "import.obsidian" => return "an imported note from Obsidian".to_owned(),
            "import.document" => return "an imported document (PDF)".to_owned(),
            "import.kindle" => return "imported Kindle reading highlights".to_owned(),
            _ => {
                return format!("imported content from {}", &stream["import.".len()..]);
            }
        }
    }
    match stream {
        None | Some("archon") => "audio transcription and screen recording".to_owned(),
        Some(_) => "captured content".to_owned(),
    }
}

fn stream_import_guidance(
    stream: Option<&str>,
    source: Option<&str>,
    kind: Option<&str>,
) -> String {
    if is_browser_route(stream, source, kind) {
        return concat!(
            "## Content Guidance\n\n",
            "This is semantic page text and change updates from web apps the owner was reading in ",
            "their browser, such as Gmail or Slack. Read it as visible page text, not audio and ",
            "not screen frames. A segment_start snapshot contains the page's visible text. Delta ",
            "rows describe text that was added or updated during the segment; remove deltas mean ",
            "text left the page. Summarize what the owner was reading, doing, and attending to."
        )
        .to_owned();
    }
    if let Some(stream) = stream
        && stream.starts_with("import.")
    {
        match stream {
            "import.chatgpt" | "import.claude" | "import.gemini" => {
                return concat!(
                    "## Content Guidance\n\n",
                    "This is an AI conversation. Summarize the key topics discussed, questions asked, ",
                    "solutions proposed, and decisions reached. Focus on what the human was trying to ",
                    "accomplish and what they learned or decided."
                )
                .to_owned();
            }
            "import.ics" => {
                return concat!(
                    "## Content Guidance\n\n",
                    "This is a calendar event. Describe the event: its purpose, participants, and any ",
                    "context from the description about why it was scheduled."
                )
                .to_owned();
            }
            "import.obsidian" => {
                return concat!(
                    "## Content Guidance\n\n",
                    "This is a note. Summarize the key ideas, references, and connections. What was the ",
                    "author thinking about and working through?"
                )
                .to_owned();
            }
            "import.document" => {
                return concat!(
                    "## Content Guidance\n\n",
                    "This is an imported document (legal, financial, medical, or personal). Extract all ",
                    "named parties and their roles (grantor, trustee, beneficiary, attorney, witness, ",
                    "agent, etc.). Produce a plain-language summary that a non-expert could understand. ",
                    "Identify key provisions, dates, conditions, obligations, and deadlines. Note any ",
                    "time-sensitive requirements (renewal dates, filing deadlines, review periods)."
                )
                .to_owned();
            }
            "import.kindle" => {
                return concat!(
                    "## Content Guidance\n\n",
                    "These are reading highlights. Describe what was being read and what the reader found ",
                    "noteworthy. What themes or ideas do these highlights capture?"
                )
                .to_owned();
            }
            _ => {
                return concat!(
                    "## Content Guidance\n\n",
                    "This is imported content. Summarize the key topics, actions, and takeaways present ",
                    "in this segment."
                )
                .to_owned();
            }
        }
    }
    match stream {
        None | Some("archon") => concat!(
            "## Live Capture Guidance\n\n",
            "ONLY report what CHANGED between screenshots or was SPOKEN in audio. ",
            "If content looks the same across frames, skip it entirely.\n\n",
            "### Your Inputs\n\n",
            "- **Screenshots**: Sampled across this segment. Compare frames — what's different?\n",
            "- **Audio**: Transcript of speech. What was said?\n\n",
            "### SKIP Entirely\n\n",
            "- Windows that look identical in first and last frame\n",
            "- Apps open but showing same content throughout\n",
            "- Background windows never brought to focus\n",
            "- Anything you'd describe as \"had open\" or \"was visible\""
        )
        .to_owned(),
        Some(_) => String::new(),
    }
}

fn calendar_days(start: NaiveDate, count: u64) -> Option<String> {
    let mut days = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let day = start.checked_add_days(Days::new(offset))?;
        days.push(day.format("%Y%m%d").to_string());
    }
    Some(days.join(" "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn weekly_window_ends_six_calendar_days_after_the_requested_start() {
        let root = tempfile::tempdir().unwrap();
        let expected_days = [
            (
                "20260830",
                "20260905",
                "20260830 20260831 20260901 20260902 20260903 20260904 20260905",
            ),
            (
                "20231231",
                "20240106",
                "20231231 20240101 20240102 20240103 20240104 20240105 20240106",
            ),
            (
                "20240225",
                "20240302",
                "20240225 20240226 20240227 20240228 20240229 20240301 20240302",
            ),
        ];

        for (start, end, days) in expected_days {
            let config = json!({"day":start, "schedule":"weekly"});
            let context = build(root.path(), config.as_object().unwrap(), None);
            assert_eq!(context["day_YYYYMMDD"], start);
            assert_eq!(context["week_end_YYYYMMDD"], end);
            assert_eq!(context["week_days_YYYYMMDD"], days);
        }

        let non_weekly_config = json!({"day":"20260830", "schedule":"daily"});
        let non_weekly_context = build(root.path(), non_weekly_config.as_object().unwrap(), None);
        assert!(!non_weekly_context.contains_key("week_days_YYYYMMDD"));
    }

    #[test]
    fn request_context_preserves_python_date_time_stream_and_duration_values() {
        let root = tempfile::tempdir().expect("root");
        let config = json!({
            "day":"20260101",
            "span":["235000_7200", "090000_30"],
            "stream":"import.custom",
            "facet":"work",
            "activity":{
                "id":"coding_1",
                "activity":"coding",
                "description":"Release work",
                "level_avg":1.0,
                "active_entities":["Mina", "Ravi"],
                "segments":["235000_7200", "090000_30"]
            }
        });
        let context = build(root.path(), config.as_object().expect("object"), None);
        assert_eq!(context["day"], "Thursday, January 01, 2026");
        assert_eq!(context["segment_start"], "9:00 AM");
        assert_eq!(context["segment_end"], "11:59 PM");
        assert_eq!(context["stream"], "import.custom");
        assert_eq!(
            context["content_description"],
            "imported content from custom"
        );
        assert_eq!(context["activity_duration"], "10");
        assert_eq!(context["activity_level"], "1.0");
        assert_eq!(context["activity_entities"], "Mina, Ravi");
        assert_eq!(
            context["activity_md_dir"],
            format!("{}/facets/work/activities/20260101/", root.path().display())
        );
    }

    #[test]
    fn activity_without_a_recorded_level_does_not_invent_medium_engagement() {
        let root = tempfile::tempdir().expect("root");
        let config = json!({
            "day":"20260101",
            "span":["090000_60"],
            "facet":"work",
            "activity":{
                "id":"reading_1",
                "activity":"reading",
                "source":"user",
                "segments":["090000_60"]
            }
        });
        let context = build(root.path(), config.as_object().expect("object"), None);
        assert!(!context.contains_key("activity_level"));
        assert!(!context["activity_context"].contains("Last engaged segment level"));
    }

    #[test]
    fn browser_stream_context_routes_by_name_and_binding() {
        let browser_description = stream_content_description(None, Some("browser"), None);
        let browser_guidance = stream_import_guidance(None, Some("browser"), None);
        let ordinary_description = stream_content_description(Some("ordinary_stream"), None, None);
        assert!(!browser_description.is_empty());
        assert!(!browser_guidance.is_empty());
        assert_ne!(browser_description, ordinary_description);

        for (stream, source, kind, browser) in [
            ("suze.browser", None, None, true),
            ("my_feed", Some("browser"), None, true),
            ("label_browser_ab12", Some("browser"), Some("browser"), true),
            ("my_feed", None, Some("browser"), true),
            ("suze.browser", Some("screen"), None, false),
            ("suze.browser", None, Some("observed"), false),
            ("label_browser", None, None, false),
            ("label_browser", Some("screen"), Some("observed"), false),
            ("my_feed", Some("import"), None, false),
        ] {
            assert_eq!(is_browser_route(Some(stream), source, kind), browser);
            let description = stream_content_description(Some(stream), source, kind);
            let guidance = stream_import_guidance(Some(stream), source, kind);
            if browser {
                assert_eq!(description, browser_description);
                assert_eq!(guidance, browser_guidance);
            } else {
                assert_eq!(description, ordinary_description);
                assert!(guidance.is_empty());
            }
        }

        let imported = stream_content_description(Some("import.custom"), None, None);
        assert!(!imported.is_empty());
        assert_ne!(imported, ordinary_description);
        assert_ne!(imported, browser_description);
        assert!(!stream_import_guidance(Some("import.custom"), None, None).is_empty());
        assert_eq!(
            stream_content_description(Some("archon"), None, None),
            stream_content_description(None, None, None)
        );
        assert!(!stream_import_guidance(Some("archon"), None, None).is_empty());
    }
}
