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
    context.insert(
        "imported_recording_rule".to_owned(),
        imported_recording_rule(stream.as_deref(), source, kind),
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
        let description = input_description(activity);
        context.insert(
            "activity_description_line".to_owned(),
            description
                .as_ref()
                .map(|description| format!("**Description:** {description}\n"))
                .unwrap_or_default(),
        );
        context.insert(
            "activity_description".to_owned(),
            description.unwrap_or_default(),
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

/// The owner's correction as a talent reads it last, when one stands.
pub(crate) fn owner_account(activity: &Map<String, Value>) -> Option<&str> {
    activity
        .get("details")
        .and_then(Value::as_str)
        .filter(|account| !account.trim().is_empty())
}

fn owner_correction(activity: &Map<String, Value>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(account) = owner_account(activity) {
        parts.push(format!("## What you told the journal\n\n{account}"));
    }
    if activity.get("playback") == Some(&Value::Bool(true)) {
        parts.push(
            "## A recording\n\nThis was a recording or video playing. You watched or listened to it; you were not at the event, and no one in it was with you."
                .to_owned(),
        );
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The correction once more, as the last thing in the request: a small model
/// follows the owner far more often when nothing comes after their words.
pub(crate) fn correction_last(activity: &Map<String, Value>) -> Option<String> {
    owner_correction(activity).map(|correction| {
        format!(
            "{correction}\n\nThis is what happened. Where anything above says otherwise, write it this way."
        )
    })
}

/// The activity's line as a talent may read it. The story step writes the line
/// from its own opening sentence, so with no correction standing, a line that
/// is the current story's opening may be a removed correction's. Read back, it
/// would keep the removed correction in the next story.
fn input_description(activity: &Map<String, Value>) -> Option<String> {
    let description = python_string(activity.get("description"));
    let own_opening = activity
        .get("story")
        .and_then(|story| story.get("body"))
        .and_then(Value::as_str)
        .and_then(crate::story::activity_line);
    let read_back = owner_correction(activity).is_none()
        && own_opening.as_deref() == Some(description.as_str());
    (!read_back).then_some(description)
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
    let description_line = input_description(activity)
        .map(|description| format!("- **Description:** {description}\n"))
        .unwrap_or_default();
    let mut parts = vec![format!(
        "## Activity Context\n- **Type:** {activity_type}\n{description_line}{engagement_line}- **Duration:** ~{} minutes ({} segments)\n- **Active Entities:** {entities}",
        estimate_duration_minutes(&segments),
        segments.len(),
    )];
    // What the owner said about the activity outranks what the journal made
    // of it. Both read the record exactly as stored.
    if let Some(account) = activity
        .get("details")
        .and_then(Value::as_str)
        .filter(|account| !account.trim().is_empty())
    {
        parts.push(format!(
            "## Your Account\nThe owner's own account of this activity, in their words. Where the transcript, the screen or the description disagrees, follow the owner. Keep quoted words exactly as they were said.\n\n{account}"
        ));
    }
    if activity.get("playback") == Some(&Value::Bool(true)) {
        parts.push(
            "## A Recording\nThe owner says this was a recording or video playing. No one heard or seen in it was present with the owner. Tell it as the owner watching or listening.".to_owned(),
        );
    }

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

const OBSIDIAN_SOURCE: &str = solstone_core_format::obsidian::OBSIDIAN_SOURCE;

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
    if source == Some(OBSIDIAN_SOURCE) {
        return "changes the owner made to their own notes in Obsidian, each note's title and path \
                followed by what they wrote in that moment"
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
            "import.evernote" => return "an imported note from Evernote".to_owned(),
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

/// Imports that hold no one speaking: chats keep their own You and Assistant labels, and
/// documents, notes, calendars, highlights, images and body data have no speakers.
const UNSPOKEN_IMPORTS: &[&str] = &[
    "chatgpt",
    "claude",
    "gemini",
    "ics",
    "obsidian",
    "evernote",
    "document",
    "kindle",
    "image",
    "text",
    "json",
    "notes",
    "strava",
    "apple_health",
    "oura",
];

const IMPORTED_RECORDING_RULE: &str = "- In a recording imported into the journal, a line is the owner's only where the transcript labels it You. Everything else in it was said by its speakers, so the speaker comes first: \"[name] [past-tense verb] [the specific thing]\", or \"A speaker [past-tense verb] [the specific thing]\" when no name is given. The owner's part is only that the recording is in their journal. Write `facets[].activity` the same way.";

/// Sense's rule for a recording imported into the journal, where the owner may not be in it at
/// all: whoever speaks is the subject, and only a line labeled You is the owner's. Live capture
/// and imports without speakers get nothing.
fn imported_recording_rule(
    stream: Option<&str>,
    source: Option<&str>,
    kind: Option<&str>,
) -> String {
    let Some(name) = stream.and_then(|stream| stream.strip_prefix("import.")) else {
        return String::new();
    };
    if is_browser_route(stream, source, kind) || UNSPOKEN_IMPORTS.contains(&name) {
        return String::new();
    }
    IMPORTED_RECORDING_RULE.to_owned()
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
            "import.obsidian" | "import.evernote" => {
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
    fn the_documented_description_variable_stays_and_empties_with_its_line() {
        let root = tempfile::tempdir().expect("root");
        let read = |activity: Value| {
            let config =
                json!({"day":"20260101","span":["090000_60"],"facet":"work","activity":activity});
            build(root.path(), config.as_object().expect("object"), None)
        };
        let told = read(json!({"id":"a","activity":"reading","description":"Read a paper."}));
        assert_eq!(told["activity_description"], "Read a paper.");
        assert_eq!(
            told["activity_description_line"],
            "**Description:** Read a paper.\n"
        );
        let read_back = read(
            json!({"id":"a","activity":"reading","description":"Read a paper.",
            "story":{"talent":"work","body":"You read a paper. It was long."}}),
        );
        assert_eq!(read_back["activity_description"], "");
        assert_eq!(read_back["activity_description_line"], "");
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

    #[test]
    fn only_an_imported_recording_gets_the_speaker_first_rule() {
        for stream in [
            "import.audio",
            "import.apple",
            "import.granola",
            "import.family_tapes",
        ] {
            assert_eq!(
                imported_recording_rule(Some(stream), None, None),
                IMPORTED_RECORDING_RULE
            );
        }
        for stream in [
            None,
            Some("archon"),
            Some("device_watch-audio"),
            Some("import.chatgpt"),
            Some("import.document"),
            Some("import.obsidian"),
            Some("import.evernote"),
            Some("import.image"),
        ] {
            assert!(imported_recording_rule(stream, None, None).is_empty());
        }
        assert!(imported_recording_rule(Some("import.audio"), Some("browser"), None).is_empty());
    }

    #[test]
    fn an_imported_evernote_note_reads_as_a_note() {
        assert_eq!(
            stream_content_description(Some("import.evernote"), None, None),
            "an imported note from Evernote"
        );
        assert_eq!(
            stream_import_guidance(Some("import.evernote"), None, None),
            stream_import_guidance(Some("import.obsidian"), None, None)
        );
        assert!(imported_recording_rule(Some("import.evernote"), None, None).is_empty());
    }

    #[test]
    fn a_note_from_a_vault_reads_as_the_owners_own_writing() {
        let description =
            stream_content_description(Some("mac_obsidian"), Some("obsidian"), Some("observer"));
        assert!(
            description.contains("their own notes in Obsidian"),
            "{description}"
        );
        assert!(imported_recording_rule(Some("mac_obsidian"), Some("obsidian"), None).is_empty());
    }
}
