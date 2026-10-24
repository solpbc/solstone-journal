// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_transcripts::{EntrySource, OWNER_VOICE, VoiceNames};

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

/// Runtime-owned config key: a Story body made of the activity's own source
/// words, set at preparation when [`source_body`] admits its source.
pub(crate) const SOURCE_BODY_KEY: &str = "_source_body";
/// Runtime-owned config key: the response says its request reached the model
/// whole, with nothing fitted away.
pub(crate) const INPUT_WHOLE_KEY: &str = "_input_whole";

/// The built-in Story talents whose source-word body was measured.
const SOURCE_BODY_TALENTS: &[&str] = &["work", "conversation"];
const MAX_QUOTE_CHARS: usize = 400;
/// The body is kept whole for readers of the activity record, including the
/// morning briefing: clipping its last words can hide a cancellation or correction
/// of an earlier claim.
pub(crate) const MAX_STORY_BODY_CHARS: usize = 1_800;

/// A Story body that quotes every word of a short activity source, framed by
/// who the producer says wrote or said it: one imported conversation, or
/// recorded speech whose speakers are framed by the journal's voice gate.
/// `None` for any other talent, source or request, which keeps the model's
/// own body.
pub(crate) fn source_body(
    name: &str,
    config: &Map<String, Value>,
    sources: &[Option<EntrySource>],
) -> Option<String> {
    if !SOURCE_BODY_TALENTS.contains(&name) || config.contains_key("messages") {
        return None;
    }
    let hook = config.get("hook")?.as_object()?;
    if hook.len() != 1 || hook.get("post")?.as_str()? != "story" {
        return None;
    }
    let transcript = config.get("transcript")?.as_str()?;
    // The model read only the measured task: this transcript and the talent's
    // own instruction. Words a request adds can ask for a body that the
    // source's words are not.
    let instruction = config.get("user_instruction")?.as_str()?;
    if crate::model_text_parts(config) != [transcript, instruction] {
        return None;
    }
    let sources = sources
        .iter()
        .map(Option::as_ref)
        .collect::<Option<Vec<_>>>()?;
    // Every line of every file is one the model reads.
    if sources.is_empty()
        || sources.iter().any(|source| {
            source.unparsed_lines != 0
                || !source
                    .chunks
                    .iter()
                    .all(|(rendered, _)| transcript.contains(rendered.as_str()))
        })
    {
        return None;
    }
    let quotes = match sources.as_slice() {
        [source] => quoted_lines(source)?,
        // Several files quote together only as recorded speech, one file per
        // segment: two files of one segment would lose their turn order. Each
        // file numbers its speakers on its own, so numbers are not carried.
        [first, rest @ ..] if rest.iter().all(|source| source.file == first.file) => sources
            .iter()
            .map(|source| spoken_lines(source, false, false))
            .collect::<Option<Vec<_>>>()?
            .concat(),
        _ => return None,
    };
    let mut body = Vec::with_capacity(quotes.len());
    for (framing, text) in quotes {
        let text = text.trim();
        if text.is_empty()
            || text.chars().count() > MAX_QUOTE_CHARS
            || text
                .chars()
                .any(|c| matches!(c, '"' | '“' | '”') || (c.is_control() && c != '\n'))
        {
            return None;
        }
        body.push(format!("{framing}: \"{text}\""));
    }
    let body = body.join(" ");
    (body.chars().count() <= MAX_STORY_BODY_CHARS).then_some(body)
}

/// Each rendered line's framing and source words, or `None` when the file is
/// not exactly one of the measured shapes. Beside those words the model may
/// read only fixed headings and producer-written ids, never other text that
/// could set the words' context: a screen frame's session and window names
/// and its description can, so screen sources keep the model's body.
fn quoted_lines(source: &EntrySource) -> Option<Vec<(String, &str)>> {
    let stream = source.stream.as_deref().unwrap_or_default();
    // The line rendered from this record, at this position.
    let rendered = |index: usize, record: &Map<String, Value>| -> Option<&str> {
        let (text, from) = source.chunks.get(index)?;
        (from == record).then_some(text.as_str())
    };
    if stream.starts_with("import.") {
        // The conversation importer writes the owner's messages as "You".
        if source.file != "conversation_transcript.jsonl" {
            return None;
        }
        let (header, messages) = source.records.split_first()?;
        let imported = header.get("imported").and_then(Value::as_object);
        if !imported.is_some_and(|imported| imported.keys().eq(["id"]))
            || header.keys().any(|key| key != "imported" && key != "model")
            || !header.get("model").is_none_or(Value::is_string)
            || messages.is_empty()
            || messages.len() != source.chunks.len()
        {
            return None;
        }
        messages
            .iter()
            .enumerate()
            .map(|(index, record)| {
                let speaker = record.get("speaker")?.as_str()?;
                let framing = match speaker {
                    "You" => "You said",
                    "Assistant" => "Your agent said",
                    _ => return None,
                };
                let start = record.get("start")?.as_str()?;
                let text = shown_text(record)?;
                if record.get("source")?.as_str()? != "import"
                    || rendered(index, record)? != format!("[{start}] (import) {speaker}: {text}")
                {
                    return None;
                }
                let sent = record
                    .get("source_ms")?
                    .as_i64()
                    .filter(|ms| *ms >= 0)
                    .and_then(chrono::DateTime::from_timestamp_millis)?
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                Some((format!("{framing} in a message dated {sent}"), text))
            })
            .collect()
    } else {
        spoken_lines(source, true, true)
    }
}

/// Recorded speech, framed only as the voice gate named it: "You" for a line
/// in the owner's recognized voice, a person named by their voice, and
/// otherwise a speaker the journal has not identified. A name the file itself
/// carries is not voice evidence, and imported recordings keep the model body.
fn spoken_lines(source: &EntrySource, alone: bool, numbered: bool) -> Option<Vec<(String, &str)>> {
    let stream = source.stream.as_deref().unwrap_or_default();
    // A header record (the recording's file name) renders nothing; every other
    // record is a statement the model read.
    let statements = source
        .records
        .iter()
        .filter(|record| record.contains_key("start") || record.contains_key("text"))
        .collect::<Vec<_>>();
    if !source.is_speech_transcript()
        || stream.starts_with("import.")
        || statements.is_empty()
        || statements.len() != source.chunks.len()
    {
        return None;
    }
    let single = alone && statements.len() == 1;
    let mut ordinal = 0_i64;
    statements
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let (text_from, from) = source.chunks.get(index)?;
            if from != record
                // One sentence may hold two speakers' words.
                || record.get("turn_boundary_suspect") == Some(&Value::Bool(true))
            {
                return None;
            }
            let start = record.get("start")?.as_str()?;
            ordinal += 1;
            let text = shown_text(record)?;
            let voiced = match record.get("sentence_id").and_then(Value::as_i64) {
                Some(id) if source.voice_named.contains(&id) => {
                    // Agent reads apply labels by statement position; frame a
                    // voice only where both readings name the same line.
                    if id != ordinal {
                        return None;
                    }
                    true
                }
                _ => false,
            };
            let (label, framing) = match record.get("speaker") {
                Some(Value::String(name)) if voiced && name == OWNER_VOICE => {
                    (format!("{name}:"), "You said".to_owned())
                }
                Some(Value::String(name)) if voiced => (format!("{name}:"), format!("{name} said")),
                Some(Value::Number(speaker)) if single => (
                    format!("Speaker {speaker}:"),
                    "An unidentified speaker said".to_owned(),
                ),
                Some(Value::Number(speaker)) if numbered => (
                    format!("Speaker {speaker}:"),
                    format!("Speaker {speaker}, whom the journal has not identified, said"),
                ),
                Some(Value::Number(speaker)) => (
                    format!("Speaker {speaker}:"),
                    "An unidentified speaker said".to_owned(),
                ),
                None if !single => (String::new(), "An unidentified speaker said".to_owned()),
                _ => return None,
            };
            let shown = if label.is_empty() {
                format!("[{start}] {text}")
            } else {
                format!("[{start}] {label} {text}")
            };
            (*text_from == shown).then_some((framing, text))
        })
        .collect()
}

/// The words a transcript line shows: its correction, else its text.
fn shown_text(record: &Map<String, Value>) -> Option<&str> {
    match record.get("corrected") {
        None | Some(Value::Null) => record.get("text")?.as_str(),
        Some(Value::String(corrected)) if corrected.is_empty() => record.get("text")?.as_str(),
        Some(Value::String(corrected)) => Some(corrected),
        Some(_) => None,
    }
}

/// The source-word body this run's Story carries: only when preparation
/// admitted one and the model read the whole request it was made from.
fn admitted_source_body(prepared: &PreparedTalent) -> Option<&str> {
    (prepared.config.get(INPUT_WHOLE_KEY) == Some(&Value::Bool(true)))
        .then(|| prepared.config.get(SOURCE_BODY_KEY)?.as_str())
        .flatten()
}

/// What a Story run returns: the stored Story when its body quotes the source,
/// else the model's response unchanged.
pub fn output(
    response: &str,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<String, StageError> {
    if admitted_source_body(prepared).is_none() {
        return Ok(response.to_owned());
    }
    match parse(response, prepared, state)? {
        ParsedOutput::Json(value) => Ok(value.to_string()),
        _ => Err(error(prepared, "story output is not JSON")),
    }
}

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
    // A valid Story over a short source carries the source's own words; its
    // topics, confidence and relations stay the model's.
    let body = admitted_source_body(prepared).map_or(body, str::to_owned);
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
    let product_owner_quotes = crate::product_names::owner_quotes(&prepared.config, &value);
    // Only the runtime says a story's input was partial; never the model.
    let object = value
        .as_object_mut()
        .ok_or_else(|| error(prepared, "story output is not an object"))?;
    object.remove("partial_input");
    if !product_owner_quotes.is_empty() {
        object.insert("_product_owner_quotes".into(), json!(product_owner_quotes));
    }
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
    apply_story_cancellable(
        root,
        talent,
        facet,
        destination_id,
        day,
        record_id,
        value,
        &solstone_core_generate::Cancellation::never(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_story_cancellable(
    root: &std::path::Path,
    talent: &str,
    facet: &str,
    destination_id: &str,
    day: &str,
    record_id: &str,
    value: &Value,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<(), String> {
    let _guard = solstone_core_facets::hold_activity_enrichment(root, facet, destination_id)
        .map_err(|error| error.to_string())?;
    crate::writers::check_cancellation(cancellation)?;
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
    .then(|| {
        owner_heard(
            root,
            day,
            &record,
            crate::transcript::voice_names(root).as_ref(),
        )
    })
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
                        .is_some_and(|quote| {
                            quoted_from(quote, &said)
                                || (value
                                    .get("_product_owner_quotes")
                                    .and_then(Value::as_array)
                                    .is_some_and(|quotes| {
                                        quotes.iter().any(|q| q.as_str() == Some(quote))
                                    })
                                    && heard.as_ref().is_some_and(|lines| {
                                        crate::product_names::canonical_owner_quote(quote, lines)
                                    }))
                        })
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
    solstone_core_facets::update_activity_record_checked(
        root,
        facet,
        day,
        record_id,
        &patch,
        "story",
        "",
        &chrono::Utc::now().to_rfc3339(),
        || crate::writers::check_cancellation(cancellation),
    )
    .map_err(|error| error.to_string())?
    .ok_or("activity no longer exists")?;
    Ok(())
}

/// What the owner said in the activity's audio, by recognized voice: `None`
/// when it has no audio transcript, empty when no line is the owner's.
/// `voices` is the journal's `voice_names`, loaded once by the caller: it reads
/// every entity, so a caller walking many records must not reload it per record.
pub(crate) fn owner_heard(
    root: &std::path::Path,
    day: &str,
    record: &Map<String, Value>,
    voices: Option<&VoiceNames>,
) -> Option<Vec<String>> {
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
                    entry.file_name().to_str().is_some_and(|name| {
                        name.ends_with("audio.jsonl")
                            || solstone_core_format::audio_archive::is_audio_archive_transcript_name(
                                name,
                            )
                    }) || solstone_core_transcripts::is_transcribe_output(&entry.path())
                });
            if !has_audio {
                continue;
            }
            audio = true;
            if !matches!(solstone_core_segment::owner_deleted(&dir), Ok(false)) {
                continue;
            }
            if let Some(voices) = voices {
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
            let prepared = PreparedTalent::new(
                talent.into(),
                Map::from_iter([
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
            );
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
        let prepared = PreparedTalent::new(
            "work".into(),
            Map::from_iter([
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
        );
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
        let prepared = PreparedTalent::new("work".into(), Map::new());
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
        let prepared = PreparedTalent::new("work".into(), Map::new());
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
        let prepared = PreparedTalent::new(
            "conversation".into(),
            Map::from_iter([
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
        );
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
    // Executes a POSIX /bin/sh one-shot fixture, not a Windows executable.
    #[cfg(unix)]
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
        let prepared = PreparedTalent::new(
            "conversation".into(),
            Map::from_iter([
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
        );
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
            &solstone_core_generate::Cancellation::never(),
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
    // Executes a POSIX /bin/sh one-shot fixture, not a Windows executable.
    #[cfg(unix)]
    #[test]
    fn a_story_from_a_clipped_request_says_its_input_was_partial() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let prepared = PreparedTalent::new(
            "work".into(),
            Map::from_iter([
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
        );
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
                &solstone_core_generate::Cancellation::never(),
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
    // Executes a POSIX /bin/sh one-shot fixture, not a Windows executable.
    #[cfg(unix)]
    #[test]
    fn a_story_request_budget_with_a_null_response_budget_does_not_set_partial_input() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let mut prepared = PreparedTalent::new(
            "work".into(),
            Map::from_iter([
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
        );
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
            &solstone_core_generate::Cancellation::never(),
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

    fn spoken(stream: &str, rows: &[(Value, &str)], voice_named: &[i64]) -> EntrySource {
        let records = rows
            .iter()
            .enumerate()
            .map(|(index, (speaker, text))| {
                let mut record = json!({"start": format!("00:00:0{index}"), "sentence_id": index as i64 + 1, "text": text});
                if !speaker.is_null() {
                    record["speaker"] = speaker.clone();
                }
                record.as_object().unwrap().clone()
            })
            .collect::<Vec<_>>();
        let chunks = records
            .iter()
            .map(|record| {
                let label = match record.get("speaker") {
                    Some(Value::Number(n)) => format!(" Speaker {n}:"),
                    Some(Value::String(name)) => format!(" {name}:"),
                    _ => String::new(),
                };
                (
                    format!(
                        "[{}]{label} {}",
                        record["start"].as_str().unwrap(),
                        record["text"].as_str().unwrap()
                    ),
                    record.clone(),
                )
            })
            .collect();
        EntrySource {
            path: std::path::PathBuf::from("capture_audio.jsonl"),
            sha256: "fixture".to_owned(),
            rendered_range: 0..0,
            chunk_offsets: Vec::new(),
            stream: Some(stream.to_owned()),
            file: "audio.jsonl".to_owned(),
            records,
            unparsed_lines: 0,
            chunks,
            voice_named: voice_named.iter().copied().collect(),
        }
    }

    #[test]
    fn spoken_lines_frame_only_what_the_voice_gate_named() {
        let source = spoken(
            "phone",
            &[
                (json!("You"), "I'll send the contract."),
                (json!("Mina Okafor"), "I filed the permit."),
                (json!(3), "Push it to May."),
                (Value::Null, "Agreed."),
            ],
            &[1, 2],
        );
        let lines = spoken_lines(&source, false, true).unwrap();
        let framing = lines
            .iter()
            .map(|(framing, _)| framing.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            framing,
            [
                "You said",
                "Mina Okafor said",
                "Speaker 3, whom the journal has not identified, said",
                "An unidentified speaker said",
            ]
        );
    }

    #[test]
    fn a_name_the_file_carries_is_not_voice_evidence() {
        // The same rows, but no sentence was named by the voice gate.
        let stored = spoken(
            "phone",
            &[(json!("You"), "I paid it."), (json!(2), "Thanks.")],
            &[],
        );
        assert!(spoken_lines(&stored, false, true).is_none());
        let named = spoken("phone", &[(json!("Priya"), "It shipped.")], &[]);
        assert!(spoken_lines(&named, true, true).is_none());
    }

    #[test]
    fn imported_recordings_and_unrendered_lines_keep_the_model_body() {
        let imported = spoken("import.plaud", &[(json!("You"), "I paid it.")], &[1]);
        assert!(spoken_lines(&imported, true, true).is_none());
        let mut changed = spoken(
            "phone",
            &[(json!(1), "It shipped."), (json!(2), "Not yet.")],
            &[],
        );
        changed.chunks[1].0.push_str(" *(happy)*");
        assert!(spoken_lines(&changed, false, true).is_none());
    }

    #[test]
    fn a_possible_speaker_change_or_shifted_sentence_id_keeps_the_model_body() {
        let mut flagged = spoken(
            "phone",
            &[(json!("You"), "Can you send it"), (json!(2), "Yes.")],
            &[1],
        );
        flagged.records[0].insert("turn_boundary_suspect".into(), Value::Bool(true));
        flagged.chunks[0].1 = flagged.records[0].clone();
        assert!(spoken_lines(&flagged, false, true).is_none());
        let mut shifted = spoken(
            "phone",
            &[(json!(2), "Hello."), (json!("You"), "I paid it.")],
            &[2],
        );
        shifted.records[1].insert("sentence_id".into(), json!(7));
        shifted.chunks[1].1 = shifted.records[1].clone();
        shifted.voice_named = [7].into();
        assert!(spoken_lines(&shifted, false, true).is_none());
    }

    #[test]
    fn several_files_quote_speakers_without_their_numbers() {
        let source = spoken(
            "phone",
            &[(json!("You"), "I booked it."), (json!(1), "Thanks.")],
            &[1],
        );
        let framing = spoken_lines(&source, false, false)
            .unwrap()
            .into_iter()
            .map(|(framing, _)| framing)
            .collect::<Vec<_>>();
        assert_eq!(framing, ["You said", "An unidentified speaker said"]);
    }

    #[test]
    fn a_recording_header_record_is_not_a_statement() {
        let mut source = spoken(
            "phone",
            &[(json!("You"), "I paid it."), (json!(2), "Thanks.")],
            &[1],
        );
        source
            .records
            .insert(0, json!({"raw": "audio.flac"}).as_object().unwrap().clone());
        let framing = spoken_lines(&source, false, true)
            .unwrap()
            .into_iter()
            .map(|(framing, _)| framing)
            .collect::<Vec<_>>();
        assert_eq!(
            framing,
            [
                "You said",
                "Speaker 2, whom the journal has not identified, said"
            ]
        );
    }

    #[test]
    fn one_numbered_utterance_keeps_its_measured_framing() {
        let one = spoken("phone", &[(json!(1), "It shipped.")], &[]);
        assert_eq!(
            spoken_lines(&one, true, true).unwrap(),
            [("An unidentified speaker said".to_owned(), "It shipped.")]
        );
    }
}
