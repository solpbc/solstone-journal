// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::{AGENT_ACTOR, NamedActor, PreparedTalent, StageError, UNKNOWN_ACTOR, stage_error};

pub(crate) const RELATIONS: &[&str] = &[
    "works-with",
    "works-at",
    "reports-to",
    "family-of",
    "knows",
    "uses",
    "created",
    "other",
];

/// An empty body is a parse error, not a silent merge. No topics is a valid
/// story: the talents are told to return `[]` when nothing is worth keeping.
/// `generate_and_write` turns a Story parse error into
/// `Finished`/`RejectedNoMutation` rather than `StageFailed`.
pub fn parse(
    output: &str,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let mut value: Value = serde_json::from_str(output.trim())
        .map_err(|_| error(prepared, "story output is not JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| error(prepared, "story output is not an object"))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "body" | "topics" | "confidence" | "relations") {
            return Err(error(
                prepared,
                &format!("story output has unexpected field: {key}"),
            ));
        }
    }
    let body = object
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| error(prepared, "story output has invalid body"))?
        .to_owned();
    let topics = object
        .get("topics")
        .and_then(Value::as_array)
        .ok_or_else(|| error(prepared, "story output has invalid topics"))?;
    let mut clean_topics = Vec::new();
    for topic in topics {
        let topic = topic
            .as_str()
            .ok_or_else(|| error(prepared, "story output has invalid topics"))?
            .trim()
            .to_lowercase();
        if !topic.is_empty() && !clean_topics.contains(&topic) && clean_topics.len() < 10 {
            clean_topics.push(topic);
        }
    }
    let confidence = object
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|number| !number.is_nan())
        .ok_or_else(|| error(prepared, "story output has invalid confidence"))?;
    let clamped = confidence.clamp(0.0, 1.0);
    if clamped != confidence {
        log::warn!("story hook: clamped confidence {confidence} to {clamped}");
    }
    if !object.get("relations").is_some_and(Value::is_array) {
        return Err(error(prepared, "story output has invalid relations"));
    }
    object.insert("body".into(), Value::String(body));
    object.insert("topics".into(), json!(clean_topics));
    object.insert("confidence".into(), json!(clamped));
    Ok(ParsedOutput::Json(value))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Json(mut value) = parsed else {
        return Err(error(prepared, "story output is not JSON"));
    };
    // Only the runtime says a story's input was partial; never the model.
    let object = value
        .as_object_mut()
        .ok_or_else(|| error(prepared, "story output is not an object"))?;
    object.remove("partial_input");
    if let Some(budget) = prepared.config.get(crate::INPUT_BUDGET_KEY) {
        object.insert(
            "partial_input".into(),
            json!({
                "dropped_entries": budget.get("dropped_entries").cloned().unwrap_or(Value::Null),
                "dropped_chars": budget.get("dropped_chars").cloned().unwrap_or(Value::Null),
            }),
        );
    }
    let facet = required(prepared, "facet")?;
    let day = required(prepared, "day")?;
    let record_id = prepared
        .config
        .get("activity")
        .and_then(Value::as_object)
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| error(prepared, "story is missing activity record id"))?;
    // Python returns "" after the mutation; CommittedNoOutput is explicit.
    Ok(CommitPlan::Write(crate::writers::WriteIntent::Story {
        destination_id: required(prepared, "destination_id")?.to_owned(),
        talent: prepared.name.clone(),
        facet: facet.to_owned(),
        day: day.to_owned(),
        record_id: record_id.to_owned(),
        value,
    }))
}

pub fn apply_story(
    root: &std::path::Path,
    talent: &str,
    facet: &str,
    destination_id: &str,
    day: &str,
    record_id: &str,
    value: &Value,
) -> Result<(), String> {
    let _guard = solstone_core_facets::hold_activity_enrichment(root, facet, destination_id)
        .map_err(|error| error.to_string())?;
    let Some(record) = solstone_core_facets::get_activity_record(root, facet, day, record_id)
        .map_err(|error| error.to_string())?
    else {
        return Err("activity no longer exists".to_owned());
    };
    let entities = crate::detected_resolution_entities(root, facet, day)?;
    let owner = crate::JournalOwner::load(root)?;
    // In a spoken conversation, "you" rests on the owner's recognized voice.
    let heard = (talent == "conversation"
        && record.get("activity").and_then(Value::as_str) == Some("meeting"))
    .then(|| owner_heard(root, day, &record))
    .flatten();
    let said = heard.as_deref().map(normalize_words).unwrap_or_default();
    // The owner's own references always resolve to the owner; "your agent" and
    // "unknown" never resolve, so neither can be matched to the owner by name.
    let resolve = |name: &str, field: &str| -> Result<Value, String> {
        match owner.actor(name) {
            NamedActor::Owner => return Ok(owner.id.clone().map_or(Value::Null, Value::String)),
            NamedActor::Agent | NamedActor::Unknown => return Ok(Value::Null),
            NamedActor::Other => {}
        }
        let result = solstone_core_entity::record_entity_resolution(
            root,
            name,
            &entities,
            json!({"kind":"facet","facet":facet}),
            json!({"lane":"talent.story","facet":facet,"day":day,"record_id":record_id,"field":field}),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        Ok(
            if matches!(
                result.outcome,
                solstone_core_entity::EntityResolutionOutcome::Resolved
            ) {
                result
                    .entity_index
                    .and_then(|index| entities[index].id.clone())
                    .map_or(Value::Null, Value::String)
            } else {
                Value::Null
            },
        )
    };
    let mut relations = Vec::new();
    for item in value["relations"].as_array().unwrap() {
        let Some(source) = item.as_object() else {
            log::warn!("story hook: skipping relation: expected object");
            continue;
        };
        if ["from", "to", "kind", "note"]
            .iter()
            .any(|field| !source.get(*field).is_some_and(Value::is_string))
        {
            log::warn!("story hook: skipping relation: missing required string field");
            continue;
        }
        let kind = source["kind"].as_str().unwrap();
        if !RELATIONS.contains(&kind)
            || (kind == "other" && source["note"].as_str().is_none_or(str::is_empty))
        {
            log::warn!("story hook: skipping relation: invalid kind");
            continue;
        }
        if source.get("quote").is_some_and(|item| !item.is_string()) {
            log::warn!("story hook: skipping relation: invalid quote");
            continue;
        }
        let mut row = source.clone();
        for (actor_field, id_field) in [("from", "from_entity_id"), ("to", "to_entity_id")] {
            if let Some(name) = row
                .get(actor_field)
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let mut name = name.as_str();
                if owner.actor(name) == NamedActor::Agent {
                    row.insert(actor_field.into(), Value::String(AGENT_ACTOR.to_owned()));
                }
                if owner.actor(name) == NamedActor::Owner && heard.is_some() {
                    if row
                        .get("quote")
                        .and_then(Value::as_str)
                        .is_some_and(|quote| quoted_from(quote, &said))
                    {
                        row.insert("owner_evidence".into(), Value::String("voice".to_owned()));
                    } else {
                        row.insert(actor_field.into(), Value::String(UNKNOWN_ACTOR.to_owned()));
                        name = UNKNOWN_ACTOR;
                    }
                }
                let resolved = resolve(name, &format!("relations.{actor_field}"))?;
                row.insert(id_field.into(), resolved);
            }
        }
        relations.push(Value::Object(row));
    }
    let mut patch = Map::new();
    let mut story = json!({"talent":talent,"body":value["body"],"topics":value["topics"],"confidence":value["confidence"]});
    // A story written from a fitted, clipped request says so on the record.
    if let Some(partial) = value.get("partial_input").filter(|item| item.is_object()) {
        story["partial_input"] = partial.clone();
    }
    patch.insert("story".into(), story);
    patch.insert("relations".into(), Value::Array(relations));
    solstone_core_facets::update_activity_record(
        root,
        facet,
        day,
        record_id,
        &patch,
        "story",
        "",
        &chrono::Utc::now().to_rfc3339(),
    )
    .map_err(|error| error.to_string())?
    .ok_or("activity no longer exists")?;
    Ok(())
}

/// What the owner said in the activity's audio, by recognized voice: `None`
/// when it has no audio transcript, empty when no line is the owner's.
pub(crate) fn owner_heard(
    root: &std::path::Path,
    day: &str,
    record: &Map<String, Value>,
) -> Option<Vec<String>> {
    let voices = crate::transcript::voice_names(root);
    let day_dir = root.join("chronicle").join(day);
    // A record that names its stream reads only that stream's audio: another
    // stream can hold a segment with the same key.
    let stream = record.get("stream").and_then(Value::as_str);
    let mut audio = false;
    let mut said = Vec::new();
    for segment in record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let mut dirs = std::fs::read_dir(&day_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| stream.is_none_or(|stream| entry.file_name() == stream))
            .map(|entry| entry.path().join(segment))
            .collect::<Vec<_>>();
        if stream.is_none() {
            dirs.push(day_dir.join(segment));
        }
        for dir in dirs.into_iter().filter(|dir| dir.is_dir()) {
            let has_audio = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with("audio.jsonl"))
                });
            if !has_audio {
                continue;
            }
            audio = true;
            if !matches!(solstone_core_segment::owner_deleted(&dir), Ok(false)) {
                continue;
            }
            if let Some(voices) = voices.as_ref() {
                said.extend(solstone_core_transcripts::owner_voice_lines(&dir, voices));
            }
        }
    }
    audio.then_some(said)
}

/// Lowercase words with punctuation dropped, one space apart.
fn normalize_words(lines: &[String]) -> String {
    lines
        .join(" ")
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A quote of at least three words that appears, word for word, in what the owner said.
fn quoted_from(quote: &str, said: &str) -> bool {
    let quote = normalize_words(&[quote.to_owned()]);
    quote.split(' ').count() >= 3 && format!(" {said} ").contains(&format!(" {quote} "))
}

fn required<'a>(prepared: &'a PreparedTalent, field: &str) -> Result<&'a str, StageError> {
    prepared
        .config
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| error(prepared, &format!("story is missing {field}")))
}
fn error(prepared: &PreparedTalent, detail: &str) -> StageError {
    stage_error("post-parse", "story", prepared, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExecutionContext;
    #[cfg(all(test, feature = "full-tests"))]
    use crate::contract::{CommitDisposition, STORY};
    #[cfg(all(test, feature = "full-tests"))]
    use crate::generate_and_write;
    use std::fs;

    #[test]
    fn spoken_you_rests_on_the_owners_recognized_voice() {
        let story_for = |method: &str, talent: &str| {
            let root = tempfile::tempdir().unwrap();
            solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None)
                .unwrap();
            let write = |path: &str, text: String| {
                let path = root.path().join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
            };
            write(
                "config/journal.json",
                json!({"identity":{"name":"Jordan Rivers"}}).to_string(),
            );
            write(
                "entities/jordan/entity.json",
                json!({"id":"jordan","name":"Jordan Rivers","type":"Person","is_principal":true})
                    .to_string(),
            );
            write(
                "facets/work/activities/20260101.jsonl",
                json!({"id":"meeting-1","activity":"meeting","segments":["090000_300"]})
                    .to_string()
                    + "\n",
            );
            write(
                "chronicle/20260101/watch/090000_300/audio.jsonl",
                r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"I will send it"}"#
                    .into(),
            );
            write(
                "chronicle/20260101/watch/090000_300/talents/speaker_labels.json",
                json!({"labels":[{"sentence_id":1,"speaker":"jordan","method":method}]})
                    .to_string(),
            );
            let prepared = PreparedTalent {
                name: talent.into(),
                config: Map::from_iter([
                    ("facet".into(), json!("work")),
                    (
                        "destination_id".into(),
                        json!(
                            solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                                .unwrap()
                        ),
                    ),
                    ("day".into(), json!("20260101")),
                    ("activity".into(), json!({"id":"meeting-1"})),
                ]),
            };
            let output = json!({
                "body":"You agreed to send the deck.",
                "topics":[],
                "confidence":0.9,
                "relations":[
                    {"from":"you","to":"Priya","kind":"works-with","note":"","quote":"I will send it"},
                    {"from":"you","to":"Priya","kind":"works-with","note":"","quote":""}
                ]
            })
            .to_string();
            let plan = commit(
                parse(&output, &prepared, &PrePostState::None).unwrap(),
                &prepared,
                &PrePostState::None,
            )
            .unwrap();
            crate::writers::apply(
                plan,
                &ExecutionContext {
                    journal: root.path().into(),
                },
            )
            .unwrap();
            solstone_core_facets::get_activity_record(root.path(), "work", "20260101", "meeting-1")
                .unwrap()
                .unwrap()["relations"]
                .clone()
        };
        // The owner's own assignment of the line is their voice, and an item
        // quoting those words is what the owner said.
        let heard = story_for("user_confirmed", "conversation");
        assert_eq!(heard[0]["from"], "you");
        assert_eq!(heard[0]["from_entity_id"], "jordan");
        assert_eq!(heard[0]["owner_evidence"], "voice");
        // An item that quotes nothing the owner said is not shown to be theirs.
        assert_eq!(heard[1]["from"], "unknown");
        assert!(heard[1]["from_entity_id"].is_null());
        assert!(heard[1].get("owner_evidence").is_none());
        // A centroid match without a confirmed voiceprint is not their voice.
        let unheard = story_for("owner_centroid", "conversation");
        for item in unheard.as_array().unwrap() {
            assert_eq!(item["from"], "unknown");
            assert!(item["from_entity_id"].is_null());
            assert!(item.get("owner_evidence").is_none());
        }
        // Work Stories are judged by what was typed and sent, not by voice.
        let work = story_for("owner_centroid", "work");
        assert_eq!(work[0]["from_entity_id"], "jordan");
        assert!(work[0].get("owner_evidence").is_none());
    }

    #[test]
    fn story_actors_resolve_the_owner_and_never_resolve_the_agent_or_unknown() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let write = |path: &str, text: String| {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "config/journal.json",
            json!({"identity":{"name":"Jordan Rivers","preferred":"Jordan","aliases":["JR"]}})
                .to_string(),
        );
        write(
            "entities/jordan/entity.json",
            json!({"id":"jordan","name":"Jordan Rivers","type":"Person","is_principal":true})
                .to_string(),
        );
        write(
            "facets/work/activities/20260101.jsonl",
            "{\"id\":\"activity-1\"}\n".into(),
        );
        // Detected names that fuzzy matching could reach from "your agent" or "unknown".
        write(
            "facets/work/entities/20260101.jsonl",
            [
                json!({"id":"agent-co","name":"Your Agent Co","type":"Organization"}),
                json!({"id":"unknown-band","name":"Unknown","type":"Organization"}),
                json!({"id":"priya","name":"Priya","type":"Person"}),
            ]
            .map(|row| row.to_string() + "\n")
            .concat(),
        );
        let prepared = PreparedTalent {
            name: "work".into(),
            config: Map::from_iter([
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let output = json!({
            "body":"You and your agent fixed the retry path.",
            "topics":[],
            "confidence":0.9,
            "relations":[
                {"from":"You","to":"Priya","kind":"works-with","note":""},
                {"from":"Your  Agent","to":"you","kind":"works-with","note":""},
                {"from":"JR","to":"your agent","kind":"works-with","note":""},
                {"from":"unknown","to":"Priya","kind":"works-with","note":""}
            ]
        })
        .to_string();
        let plan = commit(
            parse(&output, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        crate::writers::apply(
            plan,
            &ExecutionContext {
                journal: root.path().into(),
            },
        )
        .unwrap();
        let record = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        let relations = record["relations"].as_array().unwrap();
        assert_eq!(relations[0]["from_entity_id"], "jordan");
        assert_eq!(relations[0]["to_entity_id"], "priya");
        assert_eq!(relations[1]["from"], "your agent");
        assert!(relations[1]["from_entity_id"].is_null());
        assert_eq!(relations[1]["to_entity_id"], "jordan");
        assert_eq!(relations[2]["from_entity_id"], "jordan");
        assert_eq!(relations[2]["to"], "your agent");
        assert!(relations[2]["to_entity_id"].is_null());
        assert_eq!(relations[3]["from"], "unknown");
        assert!(relations[3]["from_entity_id"].is_null());
    }

    #[test]
    fn a_story_with_a_body_and_no_topics_is_kept_and_an_empty_body_is_not() {
        let prepared = PreparedTalent {
            name: "work".into(),
            config: Map::new(),
        };
        let story = |body: &str| {
            json!({"body":body,"topics":[],"confidence":0.9,"relations":[]}).to_string()
        };
        let ParsedOutput::Json(kept) = parse(
            &story("You read your inbox."),
            &prepared,
            &PrePostState::None,
        )
        .unwrap() else {
            panic!("story output is JSON");
        };
        assert_eq!(kept["topics"], json!([]));
        assert!(parse(&story("  "), &prepared, &PrePostState::None).is_err());
    }

    #[test]
    fn story_parse_accepts_exactly_four_fields_and_rejects_extra_keys() {
        let prepared = PreparedTalent {
            name: "work".into(),
            config: Map::new(),
        };
        let valid = json!({
            "body": "Valid work narrative.",
            "topics": ["code"],
            "confidence": 0.8,
            "relations": []
        })
        .to_string();
        assert!(parse(&valid, &prepared, &PrePostState::None).is_ok());

        for extra_key in [
            "commitments",
            "closures",
            "decisions",
            "partial_input",
            "unknown",
        ] {
            let mut val = json!({
                "body": "Valid narrative.",
                "topics": [],
                "confidence": 0.8,
                "relations": []
            });
            val.as_object_mut()
                .unwrap()
                .insert(extra_key.into(), json!([]));
            let err = parse(&val.to_string(), &prepared, &PrePostState::None);
            assert!(
                err.is_err(),
                "expected rejection for extra key: {extra_key}"
            );
        }
    }

    #[test]
    fn criterion_22_story_stage_uses_plain_resolution_for_written_id() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        let entity_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        // The written id is the only matching evidence: this must resolve through
        // the story stage's plain resolver, not its name-evidence sibling.
        fs::write(
            &entity_path,
            "{\"id\":\"owner-id\",\"name\":\"Different\",\"type\":\"person\"}\n",
        )
        .unwrap();
        let prepared = PreparedTalent {
            name: "conversation".into(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let value = r#"{
          "body":"A completed conversation.", "topics":["work"], "confidence":0.9,
          "relations":[{"from":"owner-id","to":"Different","kind":"works-with","note":""}]
        }"#;
        let plan = commit(
            parse(value, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        crate::writers::apply(
            plan,
            &ExecutionContext {
                journal: root.path().into(),
            },
        )
        .unwrap();
        let record = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(record["relations"][0]["from_entity_id"], "owner-id");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn criterion_7_story_validation_precedes_on_disk_mutation() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        let entity_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
        fs::write(
            &activity_path,
            "{\"id\":\"activity-1\",\"story\":{\"old\":true},\"relations\":[\"old\"]}\n",
        )
        .unwrap();
        fs::write(
            &entity_path,
            "{\"id\":\"owner-id\",\"name\":\"Different owner\",\"aka\":[\"Owner\"],\"type\":\"person\"}\n{\"id\":\"counterparty-id\",\"name\":\"Different counterparty\",\"emails\":[\"counterparty@example.com\"],\"type\":\"person\"}\n",
        )
        .unwrap();
        let prepared = PreparedTalent {
            name: "conversation".into(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let valid = r#"{
          "body":"A completed conversation.", "topics":["Work"], "confidence":0.9,
          "relations":[{"from":"Owner","to":"counterparty@example.com","kind":"works-with","note":"colleagues"}]
        }"#;
        let plan = commit(
            parse(valid, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        assert_eq!(
            crate::writers::apply(
                plan,
                &ExecutionContext {
                    journal: root.path().into()
                }
            )
            .unwrap(),
            CommitDisposition::CommittedNoOutput
        );
        let written = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        assert!(written.contains_key("story"));
        assert!(written.contains_key("relations"));
        assert!(!written.contains_key("commitments"));
        assert!(!written.contains_key("closures"));
        assert!(!written.contains_key("decisions"));
        assert_eq!(written["story"]["talent"], "conversation");
        assert_eq!(written["relations"][0]["from_entity_id"], "owner-id");
        assert_eq!(written["relations"][0]["to_entity_id"], "counterparty-id");
        assert_eq!(written["edits"].as_array().unwrap().len(), 1);
        assert_eq!(written["edits"][0]["actor"], "story");
        assert!(!root.path().join("output.md").exists());

        let unchanged = fs::read(&activity_path).unwrap();
        let client = solstone_core_generate::OneShotClient::at_path(
            crate::test_support::one_shot_stub(root.path(), "not json"),
        );
        let mut sink = Vec::new();
        let outcome = generate_and_write(
            &mut prepared.clone(),
            &ExecutionContext {
                journal: root.path().into(),
            },
            &client,
            &mut sink,
            Some((&STORY, PrePostState::None)),
        );
        let crate::RuntimeOutcome::Finished {
            disposition: CommitDisposition::RejectedNoMutation,
            degraded: Some(degraded),
            ..
        } = outcome
        else {
            panic!("a rejected story finishes degraded without mutation");
        };
        assert_eq!(degraded["reason"], "story_rejected");
        assert_eq!(degraded["detail"], "story output is not JSON");
        assert_eq!(fs::read(&activity_path).unwrap(), unchanged);
        assert!(!root.path().join("output.md").exists());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_story_from_a_clipped_request_says_its_input_was_partial() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let prepared = PreparedTalent {
            name: "work".into(),
            config: Map::from_iter([
                ("max_output_tokens".to_owned(), json!(1024)),
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        // Partial input comes from the response input_budget when clipped is true,
        // not from a request field and not from a partial_input field in the model body.
        let output = json!({
            "body":"You worked through a long terminal session.",
            "topics":["work"],
            "confidence":0.7,
            "relations":[]
        })
        .to_string();
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        let run = |input_budget: Value| {
            let client = solstone_core_generate::OneShotClient::at_path(
                crate::test_support::one_shot_stub_with_input_budget(
                    root.path(),
                    &output,
                    input_budget,
                ),
            );
            let outcome = generate_and_write(
                &mut prepared.clone(),
                &context,
                &client,
                &mut Vec::new(),
                Some((&STORY, PrePostState::None)),
            );
            assert!(matches!(
                outcome,
                crate::RuntimeOutcome::Finished {
                    disposition: CommitDisposition::CommittedNoOutput,
                    ..
                }
            ));
            solstone_core_facets::get_activity_record(root.path(), "work", "20260101", "activity-1")
                .unwrap()
                .unwrap()
        };

        let clipped = run(
            json!({"clipped":true,"dropped_chars":90_000,"dropped_entries":3,"budget_tokens":1_000}),
        );
        assert_eq!(
            clipped["story"]["partial_input"],
            json!({"dropped_entries":3,"dropped_chars":90_000})
        );

        let whole = run(Value::Null);
        assert_eq!(whole["story"]["talent"], "work");
        assert!(whole["story"].get("partial_input").is_none());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_story_request_budget_with_a_null_response_budget_does_not_set_partial_input() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let mut prepared = PreparedTalent {
            name: "work".into(),
            config: Map::from_iter([
                ("max_output_tokens".into(), json!(1024)),
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
                (
                    crate::INPUT_BUDGET_KEY.into(),
                    json!({"clipped": true, "dropped_entries": 3, "dropped_chars": 90_000}),
                ),
            ]),
        };
        let output = json!({
            "body": "You worked through a long terminal session.",
            "topics": ["work"],
            "confidence": 0.7,
            "relations": []
        })
        .to_string();
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        let client = solstone_core_generate::OneShotClient::at_path(
            crate::test_support::one_shot_stub_with_input_budget(root.path(), &output, Value::Null),
        );
        let outcome = generate_and_write(
            &mut prepared,
            &context,
            &client,
            &mut Vec::new(),
            Some((&STORY, PrePostState::None)),
        );
        assert!(matches!(
            outcome,
            crate::RuntimeOutcome::Finished {
                disposition: CommitDisposition::CommittedNoOutput,
                ..
            }
        ));
        let record = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(record["story"]["talent"], "work");
        assert!(record["story"].get("partial_input").is_none());
    }
}
