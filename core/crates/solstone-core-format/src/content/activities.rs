// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk,
    stripped_truthy_display, titleize,
};

pub(super) fn render(rel: Option<&str>, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    for record in records {
        let normalized = normalize_record(record);
        let mut lines = vec![format!("### {}", fallback_title(record))];

        if let Some(activity) = activity_type(record) {
            lines.push(format!("- Activity: {activity}"));
        }
        if let Some(facet) = stripped_truthy_display(record, "facet") {
            lines.push(format!("- Facet: {facet}"));
        }
        if let Some(day) = stripped_truthy_display(record, "day") {
            lines.push(format!("- Day: {day}"));
        }
        if let Some(time_range) = activity_time_range(record.get("segments")) {
            lines.push(format!("- Time: {time_range}"));
        }
        if let Some(level) = record.get("level_avg") {
            lines.push(format!(
                "- Last engaged segment level: {}",
                display_value(level)
            ));
        }
        if let Some(description) = stripped_truthy_display(record, "description") {
            lines.push(format!("- Description: {description}"));
        }
        if let Some(details) = stripped_truthy_display(record, "details") {
            lines.push(format!("- Details: {details}"));
        }
        if let Some(participation) = participation(record) {
            lines.push(format!("- Participation: {participation}"));
        }

        if let Some(Value::Object(story)) = record.get("story") {
            if let Some(body) = story.get("body").and_then(Value::as_str) {
                let stripped = body.trim();
                if !stripped.is_empty() {
                    lines.push(String::new());
                    lines.push(stripped.to_string());
                }
            }
            if let Some(Value::Array(topics)) = story.get("topics") {
                let topic_values: Vec<String> = topics
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|topic| !topic.is_empty())
                    .map(str::to_string)
                    .collect();
                if !topic_values.is_empty() {
                    lines.push(format!("Topics: {}", topic_values.join(", ")));
                }
            }
        }

        if json_truthy(record.get("hidden")) {
            lines.push("- Hidden: yes".to_string());
        }

        let occurrence = record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let text = with_stored_details(lines.join("\n"), record);
        chunks.push(recorded_chunk(text, occurrence, &normalized));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("activity".to_string()),
        header: Some(activity_header(rel)),
        error: None,
        warnings: Vec::new(),
    }
}

/// The byte limits an indexed entry is read back under, smallest first: the
/// owner's search view reads at 16 KiB and an agent's fetch at 64 KiB.
const READ_LIMITS: [usize; 2] = [16 * 1024, 64 * 1024];
const OMITTED: &str = "\n\nNot everything saved with this activity fit in this entry. The full activity record has all of it.";
const PARTIAL: &str = "\n\nThis story, and anything saved with it, was written from part of the activity, not all of it.";
const STORED_ARRAYS: [(&str, &str); 4] = [
    ("commitments", "Saved commitment"),
    ("closures", "Saved closure"),
    ("decisions", "Saved decision"),
    ("relations", "Saved relation"),
];

/// Append a record's saved commitments, closures, decisions and relations, and
/// whether its story saw only part of its input, after the rendered text.
///
/// The rendered text is never shortened: search over it must keep matching
/// what it matched before. Only additions are bounded, by the smallest read
/// limit the rendered text already fits, so an entry every reader could fetch
/// stays fetchable by each of them. Each entry goes in whole or not at all, a
/// later smaller one can still fit, and a skipped one leaves a notice. When the
/// rendered text has no room for that notice (and the partial-input note, when
/// the story has one), it is returned exactly as it was.
fn with_stored_details(mut text: String, record: &JsonObject) -> String {
    let Some(limit) = READ_LIMITS.into_iter().find(|limit| text.len() <= *limit) else {
        return text;
    };
    let limit = limit - OMITTED.len();
    let partial = record
        .get("story")
        .and_then(|story| story.get("partial_input"))
        .filter(|value| value.is_object());
    let reserve = if partial.is_some() { PARTIAL.len() } else { 0 };
    if text.len() + reserve > limit {
        return text;
    }
    let mut additions = Vec::new();
    if let Some(partial) = partial {
        text.push_str(PARTIAL);
        additions.push(format!("\nLeft out: {partial}"));
    }
    for (key, label) in STORED_ARRAYS {
        if let Some(Value::Array(items)) = record.get(key) {
            additions.extend(items.iter().map(|item| format!("\n\n{label}: {item}")));
        }
    }
    let mut omitted = false;
    for addition in additions {
        if text.len() + addition.len() <= limit {
            text.push_str(&addition);
        } else {
            omitted = true;
        }
    }
    if omitted {
        text.push_str(OMITTED);
    }
    text
}

fn normalize_record(record: &JsonObject) -> JsonObject {
    let mut normalized = record.clone();
    normalized.insert("title".to_string(), Value::String(source_title(record)));
    normalized.insert(
        "details".to_string(),
        Value::String(
            record
                .get("details")
                .map(display_value)
                .filter(|value| !value.is_empty())
                .unwrap_or_default(),
        ),
    );
    normalized.insert(
        "hidden".to_string(),
        Value::Bool(json_truthy(record.get("hidden"))),
    );
    normalized.insert(
        "edits".to_string(),
        Value::Array(
            record
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|edit| edit.is_object())
                .cloned()
                .collect(),
        ),
    );
    normalized
}

fn source_title(record: &JsonObject) -> String {
    if let Some(title) = stripped_truthy_display(record, "title") {
        return title;
    }
    if let Some(description) = stripped_truthy_display(record, "description") {
        return description;
    }
    if let Some(activity) = activity_type(record) {
        return titleize(&activity);
    }
    "untitled activity".to_string()
}

fn activity_header(rel: Option<&str>) -> String {
    let Some(rel) = rel else {
        return "# Activities".to_string();
    };
    let parts: Vec<&str> = rel.split('/').collect();
    let facet = parts
        .windows(3)
        .find_map(|parts| (parts[0] == "facets" && parts[2] == "activities").then_some(parts[1]))
        .unwrap_or("unknown");
    let day = rel
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"));
    match day.filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())) {
        Some(day) => format!(
            "# Activities: {facet} ({}-{}-{})",
            &day[..4],
            &day[4..6],
            &day[6..]
        ),
        None => format!("# Activities: {facet}"),
    }
}

fn fallback_title(record: &JsonObject) -> String {
    if let Some(title) = stripped_truthy_display(record, "title") {
        return title;
    }
    if let Some(description) = stripped_truthy_display(record, "description") {
        return description;
    }
    if let Some(activity) = activity_type(record) {
        return titleize(&activity);
    }
    "Untitled activity".to_string()
}

fn activity_type(record: &JsonObject) -> Option<String> {
    stripped_truthy_display(record, "activity").or_else(|| stripped_truthy_display(record, "id"))
}

fn participation(record: &JsonObject) -> Option<String> {
    let Value::Array(entries) = record.get("participation")? else {
        return None;
    };
    let names: Vec<String> = entries
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|entry| {
            stripped_truthy_display(entry, "name")
                .or_else(|| stripped_truthy_display(entry, "entity_id"))
        })
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn activity_time_range(value: Option<&Value>) -> Option<String> {
    let Value::Array(segments) = value? else {
        return None;
    };
    let first = segments.first()?.as_str()?;
    let last = segments.last()?.as_str()?;
    let (start_hour, start_minute, _, _) = parse_segment(first)?;
    let (_, _, end_second, duration) = parse_segment(last)?;
    let end_second = (end_second + duration).min(23 * 3600 + 59 * 60 + 59);
    Some(format!(
        "{start_hour:02}:{start_minute:02}-{:02}:{:02}",
        end_second / 3600,
        (end_second % 3600) / 60
    ))
}

fn parse_segment(segment: &str) -> Option<(u32, u32, u32, u32)> {
    let (time_part, length_part) = segment.split_once('_')?;
    if time_part.len() != 6
        || !time_part.bytes().all(|byte| byte.is_ascii_digit())
        || length_part.is_empty()
        || !length_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    Some((hour, minute, hour * 3600 + minute * 60 + second, duration))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{JsonObject, OMITTED, PARTIAL, normalize_record, render};

    fn object(value: Value) -> JsonObject {
        value.as_object().expect("object").clone()
    }

    fn rendered(record: &JsonObject) -> String {
        let produced = render(None, std::slice::from_ref(record));
        assert_eq!(produced.chunks.len(), 1);
        produced.chunks[0].content.clone()
    }

    /// The rendering before saved details were added: none of the fields it
    /// adds were read, so removing them reproduces it.
    fn legacy(record: &JsonObject) -> String {
        let mut record = record.clone();
        for key in ["commitments", "closures", "decisions", "relations"] {
            record.remove(key);
        }
        if let Some(Value::Object(story)) = record.get_mut("story") {
            story.remove("partial_input");
        }
        rendered(&record)
    }

    /// A record whose rendering is exactly `bytes` long before any additions.
    fn sized(mut record: JsonObject, bytes: usize) -> JsonObject {
        record.insert("story".into(), json!({"body": "x"}));
        let base = legacy(&record).len();
        assert!(bytes >= base);
        record.insert(
            "story".into(),
            json!({"body": "x".repeat(1 + bytes - base)}),
        );
        assert_eq!(legacy(&record).len(), bytes);
        record
    }

    #[test]
    fn saved_details_follow_the_unchanged_rendering() {
        let record = object(json!({
            "id": "launch-sync", "title": "Launch sync", "activity": "meeting",
            "created_at": 1_790_000_000_000_i64, "segments": ["090000_300"],
            "source_refs": [{"path": "20260918/audio.jsonl", "line": 4}],
            "custom": {"kept": true},
            "story": {"body": "  Aligned on the launch.  ", "topics": ["launch"]},
            "commitments": [{"owner": "Mina", "action": "send the notes", "context": "café  résumé"}],
            "closures": [{"action": "closed the draft"}],
            "decisions": ["ship Monday"],
            "relations": [{"from_entity_id": "mina", "to_entity_id": "pat", "role": "peer"}],
        }));
        let produced = render(None, std::slice::from_ref(&record));
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        let old = legacy(&record);
        assert!(chunk.content.starts_with(&old));
        assert_eq!(
            &chunk.content[old.len()..],
            "\n\nSaved commitment: {\"owner\":\"Mina\",\"action\":\"send the notes\",\"context\":\"café  résumé\"}\
             \n\nSaved closure: {\"action\":\"closed the draft\"}\
             \n\nSaved decision: \"ship Monday\"\
             \n\nSaved relation: {\"from_entity_id\":\"mina\",\"to_entity_id\":\"pat\",\"role\":\"peer\"}"
        );
        assert_eq!(chunk.source.as_ref(), Some(&normalize_record(&record)));
        assert_eq!(
            chunk.occurrence_time_ms.map(|time| time.0),
            Some(1_790_000_000_000)
        );
    }

    #[test]
    fn an_entry_too_big_to_add_is_skipped_and_later_ones_still_fit() {
        let mut decisions = vec![json!({"action": "huge", "context": "y".repeat(20_000)})];
        decisions.extend((0..24).map(|index| json!({"action": format!("small {index}")})));
        let record = object(json!({"title": "Review", "decisions": decisions}));
        let text = rendered(&record);
        assert!(text.starts_with(&legacy(&record)));
        assert!(text.len() <= 16 * 1024);
        assert!(!text.contains("huge"));
        for index in 0..24 {
            assert!(text.contains(&format!("{{\"action\":\"small {index}\"}}")));
        }
        assert!(text.ends_with(OMITTED));
    }

    #[test]
    fn rendering_without_room_for_the_notice_is_returned_exactly() {
        let record = object(json!({"title": "Review", "decisions": ["a"]}));
        for bytes in [
            16 * 1024 - OMITTED.len() + 1,
            16 * 1024,
            64 * 1024 - OMITTED.len() + 1,
            64 * 1024,
            64 * 1024 + 1,
            100_000,
        ] {
            let record = sized(record.clone(), bytes);
            assert_eq!(rendered(&record), legacy(&record), "{bytes}");
        }
    }

    #[test]
    fn additions_stay_under_the_smallest_limit_the_rendering_fits() {
        let decisions: Vec<Value> = (0..40)
            .map(|index| json!({"action": format!("{index} {}", "z".repeat(2_000))}))
            .collect();
        let record = object(json!({"title": "Review", "decisions": decisions}));
        for (bytes, limit) in [
            (16 * 1024 - OMITTED.len() - 200, 16 * 1024),
            (16 * 1024 + 1, 64 * 1024),
        ] {
            let record = sized(record.clone(), bytes);
            let text = rendered(&record);
            assert!(text.starts_with(&legacy(&record)));
            assert!(text.len() <= limit, "{bytes}: {}", text.len());
            assert!(text.ends_with(OMITTED));
        }
    }

    #[test]
    fn a_partial_story_says_so_even_when_its_detail_does_not_fit() {
        let partial = json!({"dropped_entries": 3, "dropped_chars": 1200});
        let record = object(
            json!({"title": "Chat", "story": {"body": "Talked.", "partial_input": partial}}),
        );
        let text = rendered(&record);
        assert_eq!(
            &text[legacy(&record).len()..],
            format!("{PARTIAL}\nLeft out: {partial}")
        );

        let huge = json!({"dropped_entries": 3, "note": "n".repeat(70_000)});
        let record =
            object(json!({"title": "Chat", "story": {"body": "Talked.", "partial_input": huge}}));
        let text = rendered(&record);
        assert_eq!(
            &text[legacy(&record).len()..],
            format!("{PARTIAL}{OMITTED}")
        );

        for partial in [Value::Null, json!(true), json!("partial")] {
            let record = object(
                json!({"title": "Chat", "story": {"body": "Talked.", "partial_input": partial}}),
            );
            assert_eq!(rendered(&record), legacy(&record));
        }
    }

    #[test]
    fn a_partial_story_without_room_for_its_note_keeps_the_exact_rendering() {
        let record = object(json!({"title": "Chat", "decisions": ["a"]}));
        let record = sized(record, 16 * 1024 - OMITTED.len() - PARTIAL.len() + 1);
        let mut partial = record.clone();
        partial.insert(
            "story".into(),
            json!({"body": record["story"]["body"], "partial_input": {"dropped_entries": 1}}),
        );
        assert_eq!(rendered(&partial), legacy(&partial));
        assert_ne!(rendered(&record), legacy(&record));
    }
}
