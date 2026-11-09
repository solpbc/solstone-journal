// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk,
    stripped_truthy_display, titleize,
};

/// Plan records the retired `schedule` talent inferred stay in their files but
/// are not indexed. An agent's read of an indexed entry skips them the same way.
pub fn is_retired_plan(record: &JsonObject) -> bool {
    record.get("source").and_then(Value::as_str) == Some("anticipated")
}

/// What a body record's `source` says.
pub const BODY_RECORD_SOURCE: &str = "body";

/// A body record refers to an account in a body window and copies none of it.
/// It is read only by composing it with that window, for a connection allowed
/// to read it, so no reader takes the file's row as it stands: it isn't thought
/// about or counted. The one thing indexed is what the owner wrote on it (see
/// [`body_record_indexed`]), and those entries come after every other record's.
pub fn is_body_record(record: &JsonObject) -> bool {
    record.get("source").and_then(Value::as_str) == Some(BODY_RECORD_SOURCE)
}

/// When a removed edit was removed.
pub const EDIT_REMOVED: &str = "removed";

/// Whether an edit was the owner's, or made from the command line.
pub fn owner_edit(edit: &JsonObject) -> bool {
    let actor = edit
        .get("actor")
        .and_then(Value::as_str)
        .unwrap_or_default();
    actor.starts_with("owner:") || actor.starts_with("cli:")
}

/// Whether an edit has been removed.
pub fn edit_removed(edit: &JsonObject) -> bool {
    edit.contains_key(EDIT_REMOVED)
}

/// Whether an edit names `field`.
pub fn edit_names(edit: &JsonObject, field: &str) -> bool {
    edit.get("fields")
        .and_then(Value::as_array)
        .is_some_and(|fields| fields.iter().any(|name| name.as_str() == Some(field)))
}

/// Whether `field` is the owner's: some edit that has not been removed, made
/// by the owner or from the command line, names it. While it is held no
/// talent writes it.
pub fn activity_field_held(record: &JsonObject, field: &str) -> bool {
    record
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .any(|edit| owner_edit(edit) && !edit_removed(edit) && edit_names(edit, field))
}

/// The fields of a body record that are the owner's words, when held.
const BODY_WORD_FIELDS: [&str; 3] = ["title", "description", "details"];

/// The line that marks an entry as a body record's: only a body record's entry
/// has it, so a reader can tell one without opening the file.
pub const BODY_ENTRY_LINE: &str = "\n- Kind: ";

/// Whether a body record has an entry: the owner holds its title, description
/// or details. The entry holds those words, its kind and its day, and nothing
/// from the source's account.
pub fn body_record_indexed(record: &JsonObject) -> bool {
    is_body_record(record)
        && BODY_WORD_FIELDS
            .iter()
            .any(|field| activity_field_held(record, field))
}

/// How many entries a day file's other records take, so that a position past
/// them is a body record's. Every reader of a position counts with this.
#[must_use]
pub fn non_body_entries(records: &[JsonObject]) -> usize {
    records
        .iter()
        .filter(|record| !is_retired_plan(record) && !is_body_record(record))
        .count()
}

/// The body record at an entry position, when the position is past the file's
/// other records.
#[must_use]
pub fn body_record_at(records: &[JsonObject], position: usize) -> Option<&JsonObject> {
    let body = position.checked_sub(non_body_entries(records))?;
    records
        .iter()
        .filter(|record| body_record_indexed(record))
        .nth(body)
}

/// What each of a day file's other records says in the index, by position, so
/// a reader can tell whether a row is one of theirs. A record a position no
/// longer holds, or holds with other words, isn't vouched for here.
#[must_use]
pub fn ordinary_entries(rel: &str, records: &[JsonObject]) -> Vec<String> {
    render(Some(rel), records)
        .chunks
        .into_iter()
        .take(non_body_entries(records))
        .map(|chunk| chunk.content.trim().to_owned())
        .collect()
}

/// Thinking's reading of a Strava workout: an activity record naming Strava's
/// stream. A workout is body data, so no daily unit counts the record and no
/// talent reads it. The owner's own readers and search still show it.
pub fn is_strava_reading(record: &JsonObject) -> bool {
    record.get("stream").and_then(Value::as_str) == Some(super::strava_body::STRAVA_STREAM)
}

pub(super) fn render(rel: Option<&str>, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    for record in records
        .iter()
        .filter(|record| !is_retired_plan(record) && !is_body_record(record))
    {
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
        // Who attended, whose names were said, and which names were only on
        // screen are different claims; a row without a known role keeps the
        // plain list.
        for (label, role) in [
            ("Attendees", Some("attendee")),
            ("In conversation", Some("heard")),
            ("Mentioned", Some("mentioned")),
            ("On screen", Some("visible")),
            ("Participation", None),
        ] {
            if let Some(names) = participation(record, role) {
                lines.push(format!("- {label}: {names}"));
            }
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
    // After every other record's, so no other entry's position moves.
    for record in records.iter().filter(|record| body_record_indexed(record)) {
        chunks.push(body_chunk(rel, record));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("activity".to_string()),
        header: Some(activity_header(rel)),
        error: None,
        warnings: Vec::new(),
    }
}

/// A body record's entry: the owner's own words, its kind and its day.
fn body_chunk(rel: Option<&str>, record: &JsonObject) -> super::IndexChunk {
    let held = |field: &str| {
        activity_field_held(record, field)
            .then(|| stripped_truthy_display(record, field))
            .flatten()
    };
    let kind = activity_type(record).unwrap_or_default();
    let mut lines = vec![format!(
        "### {}",
        held("title").unwrap_or_else(|| titleize(&kind))
    )];
    lines.push(format!("- Kind: {kind}"));
    if let Some(day) = rel.and_then(day_of_rel) {
        lines.push(format!("- Day: {day}"));
    }
    if let Some(description) = held("description") {
        lines.push(format!("- Description: {description}"));
    }
    if let Some(details) = held("details") {
        lines.push(format!("- Details: {details}"));
    }
    if json_truthy(record.get("hidden")) {
        lines.push("- Hidden: yes".to_string());
    }
    let occurrence = record
        .get("account")
        .and_then(|account| account.get("start"))
        .and_then(Value::as_str)
        .and_then(super::body_records::parse_source_time)
        .map_or(0, |start| start.timestamp_millis());
    // What search keeps beside the entry names the record, and nothing of the
    // source's account.
    let mut source = JsonObject::new();
    for key in ["id", "source", "activity"] {
        if let Some(value) = record.get(key) {
            source.insert(key.to_string(), value.clone());
        }
    }
    source.insert(
        "hidden".to_string(),
        Value::Bool(json_truthy(record.get("hidden"))),
    );
    recorded_chunk(lines.join("\n"), occurrence, &source)
}

fn day_of_rel(rel: &str) -> Option<&str> {
    rel.rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"))
        .filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The byte limits an indexed entry is read back under, smallest first: the
/// owner's search view reads at 16 KiB and an agent's fetch at 64 KiB.
const READ_LIMITS: [usize; 2] = [16 * 1024, 64 * 1024];
const OMITTED: &str = "\n\nNot everything saved with this activity fit in this entry. The full activity record has all of it.";
const STORED_ARRAYS: [(&str, &str); 1] = [("relations", "Saved relation")];

/// Append whole saved relations within the existing read limit.
/// The shared chunk renderer separately reserves space for input limitations.
fn with_stored_details(mut text: String, record: &JsonObject) -> String {
    let Some(limit) = READ_LIMITS.into_iter().find(|limit| text.len() <= *limit) else {
        return text;
    };
    let limit = limit - OMITTED.len();
    if text.len() > limit {
        return text;
    }
    let mut additions = Vec::new();
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

fn participation(record: &JsonObject, role: Option<&str>) -> Option<String> {
    const ROLES: [&str; 4] = ["attendee", "heard", "mentioned", "visible"];
    let Value::Array(entries) = record.get("participation")? else {
        return None;
    };
    let names: Vec<String> = entries
        .iter()
        .filter_map(Value::as_object)
        .filter(|entry| {
            let held = entry.get("role").and_then(Value::as_str);
            match role {
                Some(role) => held == Some(role),
                None => !held.is_some_and(|held| ROLES.contains(&held)),
            }
        })
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

    use super::{JsonObject, OMITTED, normalize_record, render};
    const PARTIAL: &str = "\n\nThis story, and anything saved with it, was written from part of the activity, not all of it.";

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
        record.remove("relations");
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
    fn participation_says_who_attended_who_was_said_and_what_was_only_on_screen() {
        let record = object(json!({
            "title": "Lyrics review", "activity": "reading",
            "participation": [
                {"name": "Mina", "role": "attendee"},
                {"name": "Iris", "role": "heard"},
                {"name": "Pat", "role": "mentioned"},
                {"name": "Cifka", "role": "visible"},
                {"name": "Suda", "role": "visible"},
                {"name": "Ola"},
            ],
        }));
        let text = rendered(&record);
        assert!(text.contains("- Attendees: Mina\n"));
        assert!(text.contains("- Mentioned: Pat\n"));
        assert!(text.contains("- On screen: Cifka, Suda\n"));
        assert!(text.contains("- Participation: Ola"));
        assert!(!text.contains("Participation: Mina"));

        let iris_lines = text
            .lines()
            .filter(|line| line.contains("Iris"))
            .collect::<Vec<_>>();
        assert_eq!(iris_lines.len(), 1);
        let iris_line = iris_lines[0];
        assert!(!iris_line.starts_with("- Attendees:"));
        assert!(!iris_line.starts_with("- Mentioned:"));
        assert!(!iris_line.starts_with("- On screen:"));
        assert!(!iris_line.starts_with("- Participation:"));
        let mentioned_line = text
            .lines()
            .find(|l| l.starts_with("- Mentioned:"))
            .unwrap();
        assert!(!mentioned_line.contains("Iris"));
        let part_line = text
            .lines()
            .find(|l| l.starts_with("- Participation:"))
            .unwrap();
        assert!(!part_line.contains("Iris"));
    }

    #[test]
    fn saved_details_follow_the_unchanged_rendering() {
        let record = object(json!({
            "id": "launch-sync", "title": "Launch sync", "activity": "meeting",
            "created_at": 1_790_000_000_000_i64, "segments": ["090000_300"],
            "source_refs": [{"path": "20260918/audio.jsonl", "line": 4}],
            "custom": {"kept": true},
            "story": {"body": "  Aligned on the launch.  ", "topics": ["launch"]},
            "relations": [{"from_entity_id": "mina", "to_entity_id": "pat", "role": "peer"}],
        }));
        let produced = render(None, std::slice::from_ref(&record));
        assert_eq!(produced.chunks.len(), 1);
        let chunk = &produced.chunks[0];
        let old = legacy(&record);
        assert!(chunk.content.starts_with(&old));
        assert_eq!(
            &chunk.content[old.len()..],
            "\n\nSaved relation: {\"from_entity_id\":\"mina\",\"to_entity_id\":\"pat\",\"role\":\"peer\"}"
        );
        assert_eq!(chunk.source.as_ref(), Some(&normalize_record(&record)));
        assert_eq!(
            chunk.occurrence_time_ms.map(|time| time.0),
            Some(1_790_000_000_000)
        );
    }

    #[test]
    fn an_entry_too_big_to_add_is_skipped_and_later_ones_still_fit() {
        let mut relations = vec![json!({"role": "huge", "context": "y".repeat(20_000)})];
        relations.extend((0..24).map(|index| json!({"role": format!("small {index}")})));
        let record = object(json!({"title": "Review", "relations": relations}));
        let text = rendered(&record);
        assert!(text.starts_with(&legacy(&record)));
        assert!(text.len() <= 16 * 1024);
        assert!(!text.contains("huge"));
        for index in 0..24 {
            assert!(text.contains(&format!("{{\"role\":\"small {index}\"}}")));
        }
        assert!(text.ends_with(OMITTED));
    }

    #[test]
    fn rendering_without_room_for_the_notice_is_returned_exactly() {
        let record = object(json!({"title": "Review", "relations": ["a"]}));
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
        let relations: Vec<Value> = (0..40)
            .map(|index| json!({"role": format!("{index} {}", "z".repeat(2_000))}))
            .collect();
        let record = object(json!({"title": "Review", "relations": relations}));
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
            format!("{PARTIAL}\nLeft out: dropped_entries: 3, dropped_chars: 1200")
        );

        let huge = json!({"dropped_entries": 3, "note": "n".repeat(70_000)});
        let record =
            object(json!({"title": "Chat", "story": {"body": "Talked.", "partial_input": huge}}));
        let text = rendered(&record);
        assert!(text.contains(PARTIAL));
        assert!(text.contains("dropped_entries: 3"));
        assert!(!text.contains(&"n".repeat(1000)));

        for partial in [Value::Null, json!(true), json!("partial")] {
            let record = object(
                json!({"title": "Chat", "story": {"body": "Talked.", "partial_input": partial}}),
            );
            assert_eq!(rendered(&record), legacy(&record));
        }
    }

    #[test]
    fn a_partial_story_reserves_space_for_its_note() {
        let record = object(json!({"title": "Chat", "relations": ["a"]}));
        let record = sized(record, 16 * 1024 - OMITTED.len() - PARTIAL.len() + 1);
        let mut partial = record.clone();
        partial.insert(
            "story".into(),
            json!({"body": record["story"]["body"], "partial_input": {"dropped_entries": 1}}),
        );
        assert!(rendered(&partial).contains(PARTIAL));
        assert!(rendered(&partial).len() <= 16 * 1024);
        assert_ne!(rendered(&partial), legacy(&partial));
        assert_ne!(rendered(&record), legacy(&record));
    }
}

#[cfg(test)]
mod body_entry_tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    fn body(id: &str, held: &[&str]) -> JsonObject {
        object(json!({
            "id": id, "source": "body", "activity": "workout", "title": "Kestrel loop",
            "description": "Written by me", "details": "With Sam",
            "edits": [{"timestamp": "t", "actor": "owner:update", "fields": held, "note": ""}],
            "account": {"key": "import.strava\u{1f}secret-key", "start": "2026-05-10T07:00:00Z",
                "end": "2026-05-10T08:00:00Z", "recorder": "secret-recorder"},
        }))
    }

    const REL: &str = "facets/body/activities/20260510.jsonl";

    #[test]
    fn a_body_entry_holds_only_the_owners_words_its_kind_and_its_day() {
        let records = [body("a", &["title"])];
        let chunks = render(Some(REL), &records).chunks;
        assert_eq!(chunks.len(), 1);
        let text = &chunks[0].content;
        assert!(text.starts_with("### Kestrel loop"), "{text}");
        assert!(
            text.contains(BODY_ENTRY_LINE.trim_start_matches('\n')),
            "{text}"
        );
        assert!(text.contains("- Day: 20260510"), "{text}");
        // What the owner didn't write, and the account, stay out.
        assert!(
            !text.contains("Written by me") && !text.contains("With Sam"),
            "{text}"
        );
        assert!(!text.contains("secret"), "{text}");
        let kept = serde_json::to_string(&chunks[0].source).unwrap();
        assert!(!kept.contains("secret"), "{kept}");
    }

    #[test]
    fn a_body_record_no_one_wrote_on_has_no_entry_and_a_removed_edit_takes_it_away() {
        let mut removed = body("a", &["title"]);
        removed["edits"][0]["removed"] = json!("t2");
        let records = [body("b", &[]), removed];
        assert!(render(Some(REL), &records).chunks.is_empty());
    }

    #[test]
    fn body_entries_follow_every_other_records_and_positions_name_them() {
        let other = object(json!({"id": "x", "activity": "meeting", "title": "Standup"}));
        let plan = object(json!({"id": "p", "source": "anticipated", "title": "Plan"}));
        let records = [
            body("bare", &[]),
            body("held", &["title"]),
            plan,
            other.clone(),
        ];
        let chunks = render(Some(REL), &records).chunks;
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content.contains("Standup"));
        assert!(chunks[1].content.contains("Kestrel loop"));
        assert_eq!(non_body_entries(&records), 1);
        assert!(body_record_at(&records, 0).is_none());
        assert_eq!(body_record_at(&records, 1).unwrap()["id"], "held");
        assert!(body_record_at(&records, 2).is_none());
    }
}
