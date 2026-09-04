// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Request-time prompt variables preserved from the Python talent runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{Map, Value};
use solstone_core_format::segment::segment_start_and_end_seconds;
use solstone_core_system_health::find_segment_dir;

pub(crate) fn build(journal: &Path, config: &Map<String, Value>) -> BTreeMap<String, String> {
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
    context.insert(
        "content_description".to_owned(),
        stream_content_description(stream.as_deref()),
    );
    context.insert(
        "import_guidance".to_owned(),
        stream_import_guidance(stream.as_deref()),
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
        context.insert(
            "activity_level".to_owned(),
            activity
                .get("level_avg")
                .map(python_value_string)
                .unwrap_or_else(|| "0.5".to_owned()),
        );
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
            activity_context(journal, day, facet, activity, &span, stream.as_deref()),
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
    let level_avg = activity
        .get("level_avg")
        .map(python_value_string)
        .unwrap_or_else(|| "0.5".to_owned());
    let numeric_level = activity
        .get("level_avg")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let level_label = if numeric_level >= 0.75 {
        "high"
    } else if numeric_level >= 0.4 {
        "medium"
    } else {
        "low"
    };
    let segments = string_array(activity.get("segments")).unwrap_or_default();
    let entities = string_array(activity.get("active_entities")).unwrap_or_default();
    let entities = if entities.is_empty() {
        "none detected".to_owned()
    } else {
        entities.join(", ")
    };
    let mut parts = vec![format!(
        "## Activity Context\n- **Type:** {activity_type}\n- **Description:** {}\n- **Engagement Level:** {level_avg} ({level_label})\n- **Duration:** ~{} minutes ({} segments)\n- **Active Entities:** {entities}",
        python_string(activity.get("description")),
        estimate_duration_minutes(&segments),
        segments.len(),
    )];

    let state_lines = span
        .iter()
        .filter_map(|segment| {
            let entry =
                load_segment_activity_state(journal, day, segment, facet, &activity_type, stream)?;
            let time_label = formatted_segment_times(segment)
                .map(|(start, end)| format!(" ({start} - {end})"))
                .unwrap_or_default();
            Some(format!(
                "### {segment}{time_label}\n{activity_type} [{}]: {}",
                python_string(entry.get("level")),
                python_string(entry.get("description")),
            ))
        })
        .collect::<Vec<_>>();
    if !state_lines.is_empty() {
        parts.push(format!(
            "## Activity State Per Segment\n\n{}",
            state_lines.join("\n\n")
        ));
    }

    parts.push(format!(
        "## Analysis Focus\nYou are analyzing ONLY the **{activity_type}** activity within the **{facet}** facet. The transcript segments may contain content from other concurrent activities (e.g., background meetings, messaging). Use the Activity State Per Segment section above to identify which content relates to this activity, and ignore unrelated content. Your analysis should only cover what happened within this specific activity."
    ));
    parts.join("\n\n")
}

fn load_segment_activity_state(
    journal: &Path,
    day: &str,
    segment: &str,
    facet: &str,
    activity_type: &str,
    stream: Option<&str>,
) -> Option<Map<String, Value>> {
    let segment = find_segment_dir(journal, day, segment, stream)?;
    let bytes = fs::read(
        segment
            .join("talents")
            .join(facet)
            .join("activity_state.json"),
    )
    .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|entry| entry.get("activity").and_then(Value::as_str) == Some(activity_type))
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

fn stream_content_description(stream: Option<&str>) -> String {
    match stream {
        None | Some("archon") => "audio transcription and screen recording".to_owned(),
        Some("import.chatgpt") => "an imported ChatGPT conversation".to_owned(),
        Some("import.claude") => "an imported Claude conversation".to_owned(),
        Some("import.gemini") => "an imported Gemini conversation".to_owned(),
        Some("import.ics") => "an imported calendar event".to_owned(),
        Some("import.obsidian") => "an imported note from Obsidian".to_owned(),
        Some("import.document") => "an imported document (PDF)".to_owned(),
        Some("import.kindle") => "imported Kindle reading highlights".to_owned(),
        Some(stream) if stream.starts_with("import.") => {
            format!("imported content from {}", &stream["import.".len()..])
        }
        Some(stream) if stream.ends_with(".browser") => {
            "semantic page text and change updates from browser web apps such as Gmail or Slack"
                .to_owned()
        }
        Some(_) => "captured content".to_owned(),
    }
}

fn stream_import_guidance(stream: Option<&str>) -> String {
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
        Some("import.chatgpt" | "import.claude" | "import.gemini") => concat!(
            "## Content Guidance\n\n",
            "This is an AI conversation. Summarize the key topics discussed, questions asked, ",
            "solutions proposed, and decisions reached. Focus on what the human was trying to ",
            "accomplish and what they learned or decided."
        )
        .to_owned(),
        Some("import.ics") => concat!(
            "## Content Guidance\n\n",
            "This is a calendar event. Describe the event: its purpose, participants, and any ",
            "context from the description about why it was scheduled."
        )
        .to_owned(),
        Some("import.obsidian") => concat!(
            "## Content Guidance\n\n",
            "This is a note. Summarize the key ideas, references, and connections. What was the ",
            "author thinking about and working through?"
        )
        .to_owned(),
        Some("import.document") => concat!(
            "## Content Guidance\n\n",
            "This is an imported document (legal, financial, medical, or personal). Extract all ",
            "named parties and their roles (grantor, trustee, beneficiary, attorney, witness, ",
            "agent, etc.). Produce a plain-language summary that a non-expert could understand. ",
            "Identify key provisions, dates, conditions, obligations, and deadlines. Note any ",
            "time-sensitive requirements (renewal dates, filing deadlines, review periods)."
        )
        .to_owned(),
        Some("import.kindle") => concat!(
            "## Content Guidance\n\n",
            "These are reading highlights. Describe what was being read and what the reader found ",
            "noteworthy. What themes or ideas do these highlights capture?"
        )
        .to_owned(),
        Some(stream) if stream.starts_with("import.") => concat!(
            "## Content Guidance\n\n",
            "This is imported content. Summarize the key topics, actions, and takeaways present ",
            "in this segment."
        )
        .to_owned(),
        Some(stream) if stream.ends_with(".browser") => concat!(
            "## Content Guidance\n\n",
            "This is semantic page text and change updates from web apps the owner was reading in ",
            "their browser, such as Gmail or Slack. Read it as visible page text, not audio and ",
            "not screen frames. A segment_start snapshot contains the page's visible text. Delta ",
            "rows describe text that was added or updated during the segment; remove deltas mean ",
            "text left the page. Summarize what the owner was reading, doing, and attending to."
        )
        .to_owned(),
        Some(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
        let context = build(root.path(), config.as_object().expect("object"));
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
}
