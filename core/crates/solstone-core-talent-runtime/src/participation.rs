// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Participation post-hook.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_system_health::find_segment_dir;

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{PreparedTalent, StageError, detected_resolution_entities, stage_error};

/// Matched lines that show a voice was heard in an activity.
const HEARD_LINES: usize = 2;

pub fn parse(
    output: &str,
    _prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "participation",
            prepared,
            "expected text output",
        ));
    };
    let Some(activity) = prepared.config.get("activity").and_then(Value::as_object) else {
        return Ok(CommitPlan::NoOutput);
    };
    let Some(facet) = prepared.config.get("facet").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    let Some(day) = prepared.config.get("day").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    let destination_id = prepared
        .config
        .get("destination_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            stage_error(
                "commit",
                "participation",
                prepared,
                "activity destination identity is missing",
            )
        })?;
    Ok(CommitPlan::Write(WriteIntent::Participation {
        destination_id: destination_id.to_owned(),
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
        activity: activity.clone(),
    }))
}

pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    destination_id: &str,
    day: &str,
    activity: &Map<String, Value>,
) -> Result<(), String> {
    apply_result_cancellable(
        journal,
        output,
        facet,
        destination_id,
        day,
        activity,
        &solstone_core_generate::Cancellation::never(),
    )
}

pub(crate) fn apply_result_cancellable(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    destination_id: &str,
    day: &str,
    activity: &Map<String, Value>,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<(), String> {
    let _guard = solstone_core_facets::hold_activity_enrichment(journal, facet, destination_id)
        .map_err(|error| error.to_string())?;
    crate::writers::check_cancellation(cancellation)?;
    // Preserve solstone/talent/participation.py:76-105: malformed model output is ignored.
    let Ok(Value::Object(data)) = serde_json::from_str(output.trim()) else {
        return Ok(());
    };
    let Some(record_id) = activity
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Ok(());
    };
    let Some(entries) = data.get("participation").and_then(Value::as_array) else {
        return Ok(());
    };
    if solstone_core_facets::get_activity_record(journal, facet, day, record_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err("activity no longer exists".to_owned());
    }
    let entities = detected_resolution_entities(journal, facet, day)?;
    let owner = crate::JournalOwner::load(journal)?;
    let origin = |field: &str| json!({"lane":"talent.participation","facet":facet,"day":day,"record_id":record_id,"field":field});
    let mut resolved = Vec::new();
    for entry in entries.iter().filter_map(Value::as_object) {
        let mut entry = entry.clone();
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Participation lists the people around the owner, never the owner.
        if owner.actor(name) == crate::NamedActor::Owner {
            continue;
        }
        if solstone_core_speaker_resolve::activity_reresolution::speaker_number(name).is_some() {
            entry.insert("entity_id".to_owned(), Value::Null);
            resolved.push(entry);
            continue;
        }
        let result = solstone_core_entity::record_entity_resolution(
            journal,
            name,
            &entities,
            json!({"kind":"facet","facet":facet}),
            origin("participation.name"),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        let entity_id = resolved_id(&result, &entities);
        if owner.id.is_some() && entity_id.as_str() == owner.id.as_deref() {
            continue;
        }
        entry.insert("entity_id".to_owned(), entity_id);
        resolved.push(entry);
    }
    // The record's own stream, when it names one: another stream can hold a
    // segment with the same key.
    let stream = activity.get("stream").and_then(Value::as_str);
    let segments = activity
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let any_meeting = segments
        .iter()
        .any(|segment| meeting_detected(journal, day, segment, stream));
    let has_numbered = resolved.iter().any(|entry| {
        entry
            .get("name")
            .and_then(Value::as_str)
            .and_then(solstone_core_speaker_resolve::activity_reresolution::speaker_number)
            .is_some()
    });
    let numbered_lines = if any_meeting && has_numbered {
        collect_numbered_lines(journal, day, &segments, stream)?
    } else {
        Vec::new()
    };
    if !segments.is_empty() && !any_meeting {
        for entry in &mut resolved {
            if entry.get("role").and_then(Value::as_str) == Some("attendee") {
                entry.insert("role".to_owned(), Value::String("mentioned".to_owned()));
            }
        }
    }
    let admitted = solstone_core_entity::load_all_journal_entities(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(solstone_core_entity::is_admissible_person)
        .collect::<Vec<_>>();
    let people = admitted
        .iter()
        .map(solstone_core_entity::JournalEntity::resolution_entity)
        .collect::<Vec<_>>();
    // A voice is heard on the evidence the talent's transcripts name it by:
    // an acoustic match or the owner's own assignment, never a name inferred
    // from what was said.
    let voices = solstone_core_transcripts::VoiceNames {
        principal: owner.id.clone().unwrap_or_default(),
        owner_voice_confirmed: false,
        people: people
            .iter()
            .filter_map(|person| Some((person.id.clone()?, person.name.clone())))
            .filter(|(id, _)| owner.id.as_ref() != Some(id))
            .collect(),
    };
    let mut voice_lines = BTreeMap::<String, usize>::new();
    let mut assigned = BTreeSet::new();
    let mut named_speakers = Vec::new();
    let mut sentences = Vec::new();
    for segment in &segments {
        sentences.extend(transcript_sentences(journal, day, segment, stream));
    }
    for &segment in &segments {
        for (speaker, by_owner) in voice_labels(journal, day, segment, stream, &voices) {
            if by_owner {
                assigned.insert(speaker.clone());
            }
            *voice_lines.entry(speaker).or_default() += 1;
        }
        named_speakers.extend(named_speakers_for_segment(journal, day, segment, stream));
    }
    // A voice was heard in the activity when two or more of its lines match
    // it, or the owner assigned one to it. A single stray match is noise.
    let attributed = voice_lines
        .into_iter()
        .filter(|(speaker, lines)| *lines >= HEARD_LINES || assigned.contains(speaker))
        .map(|(speaker, _)| speaker)
        .collect::<BTreeSet<_>>();
    let heard = people
        .iter()
        .filter(|person| person.id.as_ref().is_some_and(|id| attributed.contains(id)))
        .cloned()
        .collect::<Vec<_>>();
    for entry in &mut resolved {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(number) =
            solstone_core_speaker_resolve::activity_reresolution::speaker_number(name)
        {
            // The count is across the whole activity anyway. One activity that
            // spans two recordings can put two different voices under one number,
            // because a run restarts after RUN_GAP_S or on another stream.
            // One segment with two separately numbered audio sources can do the
            // same, because run numbers are per audio stem and only the labelled
            // audio source is named.
            if entry.get("role").and_then(Value::as_str) == Some("attendee") {
                let count = numbered_lines
                    .iter()
                    .filter(|line| line.number == number && !line.owner)
                    .count();
                if !any_meeting || count < HEARD_LINES {
                    entry.insert("role".to_owned(), Value::String("mentioned".to_owned()));
                }
            }
            continue;
        }

        if entry.get("role").and_then(Value::as_str) != Some("attendee")
            || !matches!(
                entry.get("source").and_then(Value::as_str),
                Some("voice" | "speaker_label")
            )
        {
            continue;
        }
        // A name the day's text never named resolves to the one heard voice
        // that answers to it: someone only heard is never detected.
        if entry.get("entity_id").is_none_or(Value::is_null)
            && let Some(id) = entry
                .get("name")
                .and_then(Value::as_str)
                .and_then(|name| only_person_answering(&heard, name))
        {
            entry.insert("entity_id".to_owned(), Value::String(id.to_owned()));
        }
        let entity_id = entry.get("entity_id").and_then(Value::as_str);
        let introduction = entry
            .get("name")
            .and_then(Value::as_str)
            .and_then(|name| introduced(&sentences, name));
        // An introduction keeps an unresolved name, which names no one, and a
        // resolved person only when no other admitted person answers to the
        // name as introduced: "I'm Alex" does not tell two Alexes apart.
        let corroborated = entity_id.is_some_and(|id| attributed.contains(id))
            || introduction.is_some_and(|spoken| {
                entity_id.is_none_or(|id| only_person_answering(&people, &spoken) == Some(id))
            })
            || name_resolves_to(
                &SpeakerEvidence {
                    journal,
                    people: &people,
                    facet,
                    day,
                    record_id,
                    names: &named_speakers,
                },
                entity_id,
            )?;
        if !corroborated {
            entry.insert("role".to_owned(), Value::String("mentioned".to_owned()));
        }
    }
    let shown = segments
        .iter()
        .any(|segment| has_screen_input(journal, day, segment, stream));
    // With no speech in the activity and a screen in it, nobody was heard: an
    // attendee's name came from the screen, like the tiles of a call recorded
    // without its audio. The role stands; only the source says so.
    if sentences.is_empty() && shown {
        for entry in &mut resolved {
            if matches!(
                entry.get("source").and_then(Value::as_str),
                Some("voice" | "speaker_label" | "transcript")
            ) {
                entry.insert("source".to_owned(), Value::String("screen".to_owned()));
            }
        }
    }
    if !any_meeting {
        let conversation_segments: Vec<&str> = segments
            .iter()
            .copied()
            .filter(|&segment| is_conversation_segment(journal, day, segment, stream))
            .collect();
        if !conversation_segments.is_empty() {
            let mut convo_voice_lines = BTreeMap::<String, usize>::new();
            let mut convo_assigned = BTreeSet::new();
            for &segment in &conversation_segments {
                for (speaker, by_owner) in voice_labels(journal, day, segment, stream, &voices) {
                    if by_owner {
                        convo_assigned.insert(speaker.clone());
                    }
                    *convo_voice_lines.entry(speaker).or_default() += 1;
                }
            }
            let convo_attributed = convo_voice_lines
                .into_iter()
                .filter(|(speaker, lines)| {
                    *lines >= HEARD_LINES || convo_assigned.contains(speaker)
                })
                .map(|(speaker, _)| speaker)
                .collect::<BTreeSet<_>>();
            let convo_heard = people
                .iter()
                .filter(|person| {
                    person
                        .id
                        .as_ref()
                        .is_some_and(|id| convo_attributed.contains(id))
                })
                .cloned()
                .collect::<Vec<_>>();
            for entry in &mut resolved {
                if entry
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(solstone_core_speaker_resolve::activity_reresolution::speaker_number)
                    .is_some()
                {
                    continue;
                }
                if let Some(id) = entry
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    if convo_attributed.contains(id) {
                        entry.insert("role".to_owned(), Value::String("heard".to_owned()));
                        entry.insert("source".to_owned(), Value::String("voice".to_owned()));
                    }
                } else if let Some(name) = entry.get("name").and_then(Value::as_str)
                    && let Some(id) = only_person_answering(&convo_heard, name)
                {
                    entry.insert("entity_id".to_owned(), Value::String(id.to_owned()));
                    entry.insert("role".to_owned(), Value::String("heard".to_owned()));
                    entry.insert("source".to_owned(), Value::String("voice".to_owned()));
                }
            }
        }
    }
    // A `mentioned` name the activity only showed on screen is `visible`,
    // decided from what the activity holds, not the model's pick.
    let spoken = crate::sense_grounding::Heard::new(&sentences.join("\n"));
    let sense_rows = segments
        .iter()
        .flat_map(|segment| sense_entity_rows(journal, day, segment, stream))
        .collect::<Vec<_>>();
    for entry in &mut resolved {
        if !shown || entry.get("role").and_then(Value::as_str) != Some("mentioned") {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let voiced = entry
            .get("entity_id")
            .and_then(Value::as_str)
            .is_some_and(|id| attributed.contains(id));
        // With no speech in the activity nothing was heard, whatever older
        // Sense called it. With speech, Sense must have seen it and nobody said it.
        let only_seen = sentences.is_empty()
            || (!voiced
                && !spoken.said(name)
                && !sense_rows
                    .iter()
                    .any(|(row, heard)| *heard && same_name(row, name))
                && sense_rows
                    .iter()
                    .any(|(row, heard)| !*heard && same_name(row, name)));
        if only_seen {
            entry.insert("role".to_owned(), Value::String("visible".to_owned()));
            entry.insert("source".to_owned(), Value::String("screen".to_owned()));
        }
    }
    let mut patch = Map::from_iter([(
        "participation".to_owned(),
        Value::Array(resolved.into_iter().map(Value::Object).collect()),
    )]);
    if let Some(confidence) = data.get("participation_confidence") {
        patch.insert("participation_confidence".to_owned(), confidence.clone());
    }
    patch.insert(
        "participation_partial_input".into(),
        data.get("partial_input")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or(Value::Null),
    );
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    solstone_core_facets::update_activity_record_checked(
        journal,
        facet,
        day,
        record_id,
        &patch,
        "participation",
        "updated participation",
        &timestamp,
        || crate::writers::check_cancellation(cancellation),
    )
    .map_err(|error| error.to_string())?
    .ok_or("activity no longer exists")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NumericLine {
    number: i64,
    owner: bool,
}

fn collect_numbered_lines(
    journal: &std::path::Path,
    day: &str,
    segments: &[&str],
    stream: Option<&str>,
) -> Result<Vec<NumericLine>, String> {
    let voices = crate::transcript::voice_names(journal);
    let principal = voices.as_ref().map(|v| v.principal.clone());
    let sources = solstone_core_transcripts::Sources {
        transcripts: true,
        percepts: false,
        talents: solstone_core_transcripts::TalentSource::Disabled,
        voices,
    };
    let (screen_transcript, _counts, _memory) =
        solstone_core_transcripts::cluster_span_with_memory(
            journal, day, segments, &sources, stream,
        )?;

    let mut segment_labels = BTreeMap::<std::path::PathBuf, (Option<String>, BTreeSet<i64>)>::new();
    let mut lines = Vec::new();

    for entry_source in screen_transcript.sources.iter().flatten() {
        let Some(segment_dir) = entry_source.path.parent() else {
            continue;
        };
        let (labelled_stem, owner_sentence_ids) = segment_labels
            .entry(segment_dir.to_path_buf())
            .or_insert_with(|| {
                let labelled = solstone_core_transcripts::labelled_audio_source(segment_dir);
                let mut owner_ids = BTreeSet::new();
                if let Some(ref principal_id) = principal {
                    let labels_file = segment_dir.join("talents").join("speaker_labels.json");
                    if let Ok(text) = fs::read_to_string(labels_file)
                        && let Ok(payload) = serde_json::from_str::<Value>(&text)
                        && let Some(labels) = payload.get("labels").and_then(Value::as_array)
                    {
                        for label in labels.iter().filter_map(Value::as_object) {
                            if label.get("speaker").and_then(Value::as_str)
                                == Some(principal_id.as_str())
                                && let Some(sid) = label.get("sentence_id").and_then(Value::as_i64)
                            {
                                owner_ids.insert(sid);
                            }
                        }
                    }
                }
                (labelled, owner_ids)
            });

        let file_stem = std::path::Path::new(&entry_source.file)
            .file_stem()
            .and_then(|s| s.to_str());
        let is_labelled = file_stem.is_some() && file_stem == labelled_stem.as_deref();

        for (_content, record) in &entry_source.chunks {
            let Some(number) = record.get("speaker").and_then(Value::as_i64) else {
                continue;
            };
            let owner = is_labelled
                && record
                    .get("sentence_id")
                    .and_then(Value::as_i64)
                    .is_some_and(|sid| owner_sentence_ids.contains(&sid));
            lines.push(NumericLine { number, owner });
        }
    }
    Ok(lines)
}

fn resolved_id(
    result: &solstone_core_entity::EntityResolution,
    entities: &[solstone_core_entity::EntityResolutionEntity],
) -> Value {
    if result.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved {
        return result
            .entity_index
            .and_then(|index| entities[index].id.clone())
            .map_or(Value::Null, Value::String);
    }
    Value::Null
}

/// A segment's named speakers, resolved against every admitted person in the
/// journal rather than the day's detected entities, so a name that two people
/// answer to stays ambiguous even when only one of them was detected.
struct SpeakerEvidence<'a> {
    journal: &'a std::path::Path,
    people: &'a [solstone_core_entity::EntityResolutionEntity],
    facet: &'a str,
    day: &'a str,
    record_id: &'a str,
    names: &'a [String],
}

fn name_resolves_to(
    evidence: &SpeakerEvidence<'_>,
    entity_id: Option<&str>,
) -> Result<bool, String> {
    for name in evidence.names {
        let result = solstone_core_entity::record_entity_resolution(
            evidence.journal,
            name,
            evidence.people,
            json!({"kind":"facet","facet":evidence.facet}),
            json!({"lane":"talent.participation","facet":evidence.facet,"day":evidence.day,"record_id":evidence.record_id,"field":"speaker.name"}),
            90.0,
            false,
        )
        .map_err(|error| error.to_string())?;
        if result.outcome == solstone_core_entity::EntityResolutionOutcome::Ambiguous {
            continue;
        }
        let resolved_id = resolved_id(&result, evidence.people);
        if entity_id.is_some_and(|id| resolved_id.as_str() == Some(id)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn meeting_detected(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> bool {
    segment_talent_json(journal, day, segment, stream, "sense.json")
        .and_then(|value| value.get("meeting_detected").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Whether a segment holds screen input: a screen or terminal recording.
fn has_screen_input(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> bool {
    find_segment_dir(journal, day, segment, stream)
        .and_then(|directory| fs::read_dir(directory).ok())
        .is_some_and(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .any(|name| name.ends_with("screen.jsonl"))
        })
}

fn has_visual_files(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> bool {
    find_segment_dir(journal, day, segment, stream)
        .and_then(|directory| fs::read_dir(directory).ok())
        .is_some_and(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .any(|name| {
                    name.ends_with("screen.jsonl")
                        || name == "browser_pages.jsonl"
                        || name.ends_with("_desktop_activity.jsonl")
                        || name.ends_with("_protected_video.jsonl")
                })
        })
}

fn is_conversation_segment(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> bool {
    let Some(sense) = segment_talent_json(journal, day, segment, stream, "sense.json") else {
        return false;
    };
    if sense.get("content_type").and_then(Value::as_str) != Some("social") {
        return false;
    }
    if transcript_sentences(journal, day, segment, stream).is_empty() {
        return false;
    }
    if has_visual_files(journal, day, segment, stream) {
        return false;
    }
    true
}

/// A segment's Sense entity names, each with whether it was heard. A row is
/// seen when Sense called it `visible`, or, in Sense written before `visible`
/// existed, `mentioned` from the screen.
fn sense_entity_rows(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Vec<(String, bool)> {
    segment_talent_json(journal, day, segment, stream, "sense.json")
        .and_then(|sense| sense.get("entities").and_then(Value::as_array).cloned())
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let name = row.get("name").and_then(Value::as_str)?.trim().to_owned();
            let role = row.get("role").and_then(Value::as_str).unwrap_or_default();
            let source = row
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let seen = role == "visible" || (role == "mentioned" && source == "screen");
            (!name.is_empty()).then_some((name, !seen))
        })
        .collect()
}

fn same_name(left: &str, right: &str) -> bool {
    let fold = |name: &str| crate::fold_name(name);
    fold(left) == fold(right)
}

/// The voice of each line in a segment that a voice match or the owner's own
/// assignment names, and whether the owner assigned it.
fn voice_labels(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    voices: &solstone_core_transcripts::VoiceNames,
) -> Vec<(String, bool)> {
    segment_talent_json(journal, day, segment, stream, "speaker_labels.json")
        .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_object)
        .filter(|label| voices.name_for(label).is_some())
        .filter_map(|label| {
            let speaker = label.get("speaker").and_then(Value::as_str)?;
            let by_owner = label
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|method| method.starts_with("user_"));
            voices
                .people
                .contains_key(speaker)
                .then(|| (speaker.to_owned(), by_owner))
        })
        .collect()
}

fn named_speakers_for_segment(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Vec<String> {
    segment_talent_json(journal, day, segment, stream, "speakers.json")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|speaker| !speaker.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// The name as a sentence introduces it explicitly: "I'm Dana", "I am Dana",
/// "my name is Dana", "this is Dana" or "Dana here". The full name when a
/// sentence says it, otherwise the first name. Someone who introduces
/// themselves in the activity was there, which is what a speaker label would
/// otherwise have to show.
fn introduced(sentences: &[String], name: &str) -> Option<String> {
    let words = name
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let first = *words.first().filter(|word| word.chars().count() >= 2)?;
    let full = words.join(" ");
    [full.as_str(), first]
        .into_iter()
        .find(|spoken| {
            let spoken = spoken
                .split(' ')
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(r"\s+");
            regex::Regex::new(&format!(
                r"(?i)\b(?:i'?m|i am|my name is|my name's|this is)\s+{spoken}\b|\b{spoken}\s+here\b"
            ))
            .is_ok_and(|pattern| sentences.iter().any(|sentence| pattern.is_match(sentence)))
        })
        .map(str::to_owned)
}

/// The one admitted person who answers to a spoken name, if only one does. A
/// person answers to their name and aliases, and to the first word of each.
fn only_person_answering<'a>(
    people: &'a [solstone_core_entity::EntityResolutionEntity],
    spoken: &str,
) -> Option<&'a str> {
    let spoken = spoken.to_lowercase();
    let mut answering = people.iter().filter(|person| {
        std::iter::once(&person.name)
            .chain(&person.aka)
            .map(|known| known.trim().to_lowercase())
            .any(|known| {
                known == spoken
                    || known.split_whitespace().next().is_some_and(|word| {
                        word.trim_matches(|c: char| !c.is_alphanumeric()) == spoken
                    })
            })
    });
    let person = answering.next()?;
    answering
        .next()
        .is_none()
        .then_some(person.id.as_deref())
        .flatten()
}

/// The spoken sentences of a segment's audio transcripts.
fn transcript_sentences(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Vec<String> {
    let Some(directory) = find_segment_dir(journal, day, segment, stream) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.strip_suffix(".jsonl").is_some_and(|stem| {
                stem == "audio"
                    || stem.ends_with("_audio")
                    || solstone_core_format::audio_archive::is_audio_archive_part_stem(stem)
            })
        })
        .collect::<Vec<_>>();
    names.sort();
    names
        .iter()
        .filter_map(|name| fs::read_to_string(directory.join(name)).ok())
        .flat_map(|text| {
            text.lines()
                .skip(1)
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|row| row.get("text").and_then(Value::as_str).map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn segment_talent_json(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    name: &str,
) -> Option<Value> {
    let directory = find_segment_dir(journal, day, segment, stream)?;
    serde_json::from_str(&fs::read_to_string(directory.join("talents").join(name)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::*;

    const DAY: &str = "20260101";
    const FACET: &str = "work";
    const SEGMENT: &str = "090000_60";

    fn activity() -> Map<String, Value> {
        json!({"id":"activity","segments":[SEGMENT]})
            .as_object()
            .expect("activity is an object")
            .clone()
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().expect("fixture path has a parent"))
            .expect("fixture parent");
        fs::write(path, value.to_string()).expect("fixture JSON");
    }

    fn write_entity(root: &Path, id: &str, name: &str, entity_type: &str) {
        write_json(
            &root.join(format!("entities/{id}/entity.json")),
            json!({"id":id,"name":name,"type":entity_type}),
        );
    }

    fn write_participation_fixture(root: &Path, labels: Value, speakers: Value) {
        solstone_core_facets::create_facet(root, FACET, "Work", "", "", "", None).unwrap();
        write_entity(root, "person", "Ada Lovelace", "Person");
        write_entity(root, "tool", "Deploy Bot", "Tool");
        write_entity(root, "project", "Atlas", "Project");
        let detected = [
            json!({"id":"person","name":"Ada Lovelace","type":"Person"}),
            json!({"id":"tool","name":"Deploy Bot","type":"Tool"}),
            json!({"id":"project","name":"Atlas","type":"Project"}),
        ]
        .into_iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let detected_path = root.join(format!("facets/{FACET}/entities/{DAY}.jsonl"));
        fs::create_dir_all(detected_path.parent().expect("detected parent"))
            .expect("detected directory");
        fs::write(detected_path, format!("{detected}\n")).expect("detected entities");
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        fs::create_dir_all(activity_path.parent().expect("activity parent"))
            .expect("activity directory");
        let activity_line = json!({"id":"activity","segments":[SEGMENT]}).to_string();
        fs::write(activity_path, format!("{activity_line}\n")).expect("activity");
        let talents = root.join(format!("chronicle/{DAY}/{SEGMENT}/talents"));
        fs::create_dir_all(&talents).expect("talents directory");
        write_json(
            talents.join("sense.json").as_path(),
            json!({"meeting_detected":true}),
        );
        write_json(talents.join("speaker_labels.json").as_path(), labels);
        write_json(talents.join("speakers.json").as_path(), speakers);
    }

    fn apply(root: &Path, entry: Value) -> Value {
        apply_result(
            root,
            &json!({"participation":[entry]}).to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root, FACET).unwrap(),
            DAY,
            &activity(),
        )
        .expect("participation applies");
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let activity: Value = serde_json::from_str(
            fs::read_to_string(activity_path)
                .expect("activity reads")
                .lines()
                .next()
                .expect("activity row"),
        )
        .expect("activity parses");
        activity["participation"][0].clone()
    }

    #[test]
    fn partial_participation_replacement_preserves_independent_story_marker() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!({}));
        let path = root
            .path()
            .join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let mut row = activity();
        row.insert(
            "story".into(),
            json!({"body":"Saved story","partial_input":{"dropped_entries":8}}),
        );
        fs::write(&path, format!("{}\n", Value::Object(row.clone()))).unwrap();
        let original = row.clone();
        for clipped in [true, false, true] {
            let mut output = json!({"participation":[]});
            if clipped {
                output["partial_input"] = json!({"dropped_entries":2});
            }
            apply_result(
                root.path(),
                &output.to_string(),
                FACET,
                &solstone_core_facets::observe_facet_write_identity(root.path(), FACET).unwrap(),
                DAY,
                &original,
            )
            .unwrap();
            row = serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap())
                .unwrap();
            assert_eq!(row["story"]["partial_input"]["dropped_entries"], 8);
            assert_eq!(
                row.get("participation_partial_input")
                    .is_some_and(Value::is_object),
                clipped
            );
        }
    }

    #[test]
    fn participation_never_lists_the_owner() {
        let root = tempfile::tempdir().expect("tempdir");
        write_participation_fixture(root.path(), json!({}), json!({}));
        write_json(
            &root.path().join("config/journal.json"),
            json!({"identity":{"name":"Jordan Rivers","preferred":"Jordan"}}),
        );
        write_json(
            &root.path().join("entities/jordan/entity.json"),
            json!({"id":"jordan","name":"Jordan Rivers","aka":["J. Rivers"],"type":"Person","is_principal":true}),
        );
        let detected = root
            .path()
            .join(format!("facets/{FACET}/entities/{DAY}.jsonl"));
        let mut rows = fs::read_to_string(&detected).expect("detected entities");
        rows.push_str(&format!(
            "{}\n",
            json!({"id":"jordan","name":"Jordan Rivers","type":"Person"})
        ));
        fs::write(&detected, rows).expect("detected entities");
        let entry = |name: &str| json!({"name":name,"role":"mentioned","source":"screen","confidence":0.5,"context":"","entity_id":null});
        apply_result(
            root.path(),
            &json!({"participation":[entry("you"), entry("Jordan"), entry("Jordan Rivers"), entry("Ada Lovelace")]})
                .to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root.path(), FACET).unwrap(),
            DAY,
            &activity(),
        )
        .expect("participation applies");
        let activity_path = root
            .path()
            .join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let activity: Value = serde_json::from_str(
            fs::read_to_string(activity_path)
                .expect("activity reads")
                .lines()
                .next()
                .expect("activity row"),
        )
        .expect("activity parses");
        let names = activity["participation"]
            .as_array()
            .expect("participation")
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Ada Lovelace"]);
    }

    #[test]
    fn missing_segment_resolution_is_non_creating() {
        // Derived from solstone/talent/participation.py:24-30 and
        // solstone/think/cluster.py:749-764; Python is not runnable here.
        let root = tempfile::tempdir().unwrap();
        assert!(find_segment_dir(root.path(), "20260101", "090000_60", None).is_none());
        assert!(!root.path().join("chronicle/20260101").exists());
    }

    fn write_transcript(root: &Path, sentences: &[&str]) {
        let mut text = json!({"raw":"fixture"}).to_string() + "\n";
        for sentence in sentences {
            text += &(json!({"start":"00:00:01","text":sentence}).to_string() + "\n");
        }
        fs::write(
            root.join(format!("chronicle/{DAY}/{SEGMENT}/audio.jsonl")),
            text,
        )
        .expect("transcript");
    }

    #[test]
    fn a_person_who_introduces_themselves_stays_an_attendee_without_a_label() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
        write_transcript(
            root.path(),
            &["Hi everyone.", "My name is Ada, I run the lab."],
        );

        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["role"], "attendee");
    }

    #[test]
    fn without_an_introduction_or_a_label_a_voice_attendee_is_still_demoted() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
        write_transcript(
            root.path(),
            &["Ada said she would send it.", "This is Atlas."],
        );

        let person = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"voice"}),
        );
        assert_eq!(person["role"], "mentioned");
        // Only a person is kept by an introduction, never a project.
        let project = apply(
            root.path(),
            json!({"name":"Atlas","role":"attendee","source":"voice"}),
        );
        assert_eq!(project["role"], "mentioned");
    }

    /// Two admitted people answer to "Alex"; only `detected` reached the day's
    /// detected entities.
    fn write_alex_fixture(root: &Path, detected: &[&str], labels: Value, speakers: Value) {
        write_participation_fixture(root, labels, speakers);
        let people = [
            json!({"id":"alex-stone","name":"Alex Stone","aka":["Alex"],"type":"Person"}),
            json!({"id":"alex-reed","name":"Alex Reed","aka":["Alex"],"type":"Person"}),
        ];
        let detected_path = root.join(format!("facets/{FACET}/entities/{DAY}.jsonl"));
        let mut rows = fs::read_to_string(&detected_path).expect("detected entities");
        for person in people {
            let id = person["id"].as_str().expect("fixture id");
            write_json(
                &root.join(format!("entities/{id}/entity.json")),
                person.clone(),
            );
            if detected.contains(&id) {
                rows.push_str(&format!("{person}\n"));
            }
        }
        fs::write(detected_path, rows).expect("detected entities");
    }

    fn alex_stone() -> Value {
        json!({"name":"Alex Stone","role":"attendee","source":"voice"})
    }

    #[test]
    fn a_first_name_two_people_share_does_not_make_either_an_attendee() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_alex_fixture(
            root.path(),
            &["alex-stone", "alex-reed"],
            json!({"labels":[]}),
            json!(["Alex"]),
        );
        write_transcript(
            root.path(),
            &["I am Alex. My surname is not stated in this recording."],
        );

        let entry = apply(root.path(), alex_stone());

        assert_eq!(entry["entity_id"], "alex-stone");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn a_shared_speaker_name_stays_ambiguous_when_only_one_was_detected() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_alex_fixture(
            root.path(),
            &["alex-stone"],
            json!({"labels":[]}),
            json!(["Alex"]),
        );

        let entry = apply(root.path(), alex_stone());

        assert_eq!(entry["entity_id"], "alex-stone");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn evidence_that_tells_the_two_apart_keeps_the_attendee() {
        let cases = [
            // The full name, introduced.
            (json!([]), json!([]), "Hi, I'm Alex Stone from the lab."),
            // A voice bound to one of them.
            (
                json!([{"speaker":"alex-stone","method":"acoustic"},{"speaker":"alex-stone","method":"acoustic"}]),
                json!(["Alex"]),
                "I am Alex.",
            ),
            // A named speaker only one of them answers to.
            (json!([]), json!(["Alex Stone"]), "I am Alex."),
        ];
        for (labels, speakers, sentence) in cases {
            let root = tempfile::tempdir().expect("temporary journal");
            write_alex_fixture(
                root.path(),
                &["alex-stone", "alex-reed"],
                json!({"labels":labels}),
                speakers,
            );
            write_transcript(root.path(), &[sentence]);

            let entry = apply(root.path(), alex_stone());

            assert_eq!(entry["entity_id"], "alex-stone", "{sentence}");
            assert_eq!(entry["role"], "attendee", "{sentence}");
        }
    }

    #[test]
    fn an_unresolved_shared_name_keeps_its_introduction_and_names_no_one() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_alex_fixture(
            root.path(),
            &["alex-stone", "alex-reed"],
            json!({"labels":[]}),
            json!([]),
        );
        write_transcript(root.path(), &["I am Alex."]);

        let entry = apply(
            root.path(),
            json!({"name":"Alex","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], Value::Null);
        assert_eq!(entry["role"], "attendee");
    }

    #[test]
    fn a_person_only_talked_about_is_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
        write_transcript(
            root.path(),
            &["Ada Lovelace was only mentioned. Ada is absent and did not join this call."],
        );

        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn a_person_named_in_an_imported_conversation_is_mentioned_and_you_is_dropped() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
        write_json(
            &root.path().join("config/journal.json"),
            json!({"identity":{"name":"Jordan Rivers","preferred":"Jordan"}}),
        );
        write_json(
            &root
                .path()
                .join(format!("chronicle/{DAY}/{SEGMENT}/talents/sense.json")),
            json!({"meeting_detected":false}),
        );
        write_transcript(
            root.path(),
            &[
                "Please draft the patch.",
                "I drafted it. Ada was only mentioned and did not join.",
            ],
        );

        apply_result(
            root.path(),
            &json!({"participation":[
                {"name":"You","role":"attendee","source":"voice"},
                {"name":"Ada Lovelace","role":"attendee","source":"voice"},
            ]})
            .to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root.path(), FACET).unwrap(),
            DAY,
            &activity(),
        )
        .expect("participation applies");
        let activity_path = root
            .path()
            .join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let activity: Value = serde_json::from_str(
            fs::read_to_string(activity_path)
                .expect("activity reads")
                .lines()
                .next()
                .expect("activity row"),
        )
        .expect("activity parses");

        assert_eq!(
            activity["participation"],
            json!([{"name":"Ada Lovelace","role":"mentioned","source":"voice","entity_id":"person"}])
        );
    }

    #[test]
    fn speaker_labeled_tool_demotes_to_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(
            root.path(),
            json!({"labels":[{"speaker":"tool","method":"acoustic"},{"speaker":"tool","method":"acoustic"}]}),
            json!([]),
        );

        let entry = apply(
            root.path(),
            json!({"name":"Deploy Bot","role":"attendee","source":"speaker_label"}),
        );

        assert_eq!(entry["entity_id"], "tool");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn raw_named_project_demotes_to_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!(["Atlas"]));

        let entry = apply(
            root.path(),
            json!({"name":"Atlas","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], "project");
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn speaker_labeled_person_remains_corroborated() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(
            root.path(),
            json!({"labels":[{"speaker":"person","method":"acoustic"},{"speaker":"person","method":"acoustic"}]}),
            json!([]),
        );

        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"speaker_label"}),
        );

        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["role"], "attendee");
    }

    /// Grace is admitted but never named in the day's text, so she is not
    /// among the day's detected entities.
    fn write_heard_fixture(root: &Path, labels: Value) {
        write_participation_fixture(root, json!({ "labels": labels }), json!([]));
        write_entity(root, "grace", "Grace Hopper", "Person");
    }

    #[test]
    fn a_heard_voice_the_day_never_named_stays_an_attendee() {
        for name in ["Grace Hopper", "Grace"] {
            let root = tempfile::tempdir().expect("temporary journal");
            write_heard_fixture(root.path(), grace_heard_twice());

            let entry = apply(
                root.path(),
                json!({"name":name,"role":"attendee","source":"voice"}),
            );

            assert_eq!(entry["entity_id"], "grace", "{name}");
            assert_eq!(entry["role"], "attendee", "{name}");
        }
    }

    #[test]
    fn a_name_two_heard_voices_answer_to_names_no_one() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_heard_fixture(
            root.path(),
            json!([
                {"speaker":"grace","method":"acoustic"},
                {"speaker":"grace","method":"acoustic"},
                {"speaker":"grace-brewster","method":"acoustic_cluster"},
                {"speaker":"grace-brewster","method":"acoustic_cluster"},
            ]),
        );
        write_entity(root.path(), "grace-brewster", "Grace Brewster", "Person");

        let entry = apply(
            root.path(),
            json!({"name":"Grace","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], Value::Null);
        assert_eq!(entry["role"], "mentioned");
    }

    #[test]
    fn a_name_inferred_from_what_was_said_is_not_a_heard_voice() {
        for (speaker, name) in [("grace", "Grace Hopper"), ("person", "Ada Lovelace")] {
            let root = tempfile::tempdir().expect("temporary journal");
            write_heard_fixture(
                root.path(),
                json!([
                    {"speaker":speaker,"method":"contextual"},
                    {"speaker":speaker,"method":"contextual"},
                ]),
            );

            let entry = apply(
                root.path(),
                json!({"name":name,"role":"attendee","source":"voice"}),
            );

            assert_eq!(entry["role"], "mentioned", "{name}");
        }
    }

    fn grace_heard_twice() -> Value {
        json!([
            {"speaker":"grace","method":"acoustic"},
            {"speaker":"grace","method":"acoustic_cluster"},
        ])
    }

    #[test]
    fn one_stray_voice_match_is_not_attending() {
        for name in ["Grace Hopper", "Ada Lovelace"] {
            let root = tempfile::tempdir().expect("temporary journal");
            write_heard_fixture(
                root.path(),
                json!([
                    {"speaker":"grace","method":"acoustic"},
                    {"speaker":"person","method":"acoustic"},
                ]),
            );

            let entry = apply(
                root.path(),
                json!({"name":name,"role":"attendee","source":"voice"}),
            );

            assert_eq!(entry["role"], "mentioned", "{name}");
        }
    }

    #[test]
    fn one_line_the_owner_assigned_is_attending() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_heard_fixture(
            root.path(),
            json!([{"speaker":"grace","method":"user_assigned"}]),
        );

        let entry = apply(
            root.path(),
            json!({"name":"Grace Hopper","role":"attendee","source":"voice"}),
        );

        assert_eq!(entry["entity_id"], "grace");
        assert_eq!(entry["role"], "attendee");
    }

    fn voice(axis: usize, noise: f32) -> Vec<f32> {
        let mut row = vec![0.0; 256];
        row[..4].fill(noise);
        row[axis] = 1.0;
        row
    }

    fn write_embedded_segment(dir: &Path, lines: &[(i64, Vec<f32>)]) {
        use solstone_core_npy::write_npy;
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};

        fs::create_dir_all(dir).unwrap();
        let mut jsonl = String::from("{\"raw\":\"audio.m4a\"}\n");
        for (index, (speaker, _)) in lines.iter().enumerate() {
            jsonl.push_str(&format!(
                "{{\"start\":\"00:00:{index:02}\",\"speaker\":{speaker},\"sentence_id\":{},\"text\":\"line\"}}\n",
                index + 1
            ));
        }
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();

        let mut payload = Vec::new();
        let mut ids = Vec::new();
        for (index, (_, embedding)) in lines.iter().enumerate() {
            for value in embedding {
                payload.extend_from_slice(&value.to_le_bytes());
            }
            ids.extend_from_slice(&(index as i32 + 1).to_le_bytes());
        }
        let mut zip = ZipWriter::new(fs::File::create(dir.join("audio.npz")).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("embeddings.npy", options).unwrap();
        zip.write_all(&write_npy(
            "<f4",
            &format!("({}, 256)", lines.len()),
            &payload,
        ))
        .unwrap();
        zip.start_file("statement_ids.npy", options).unwrap();
        zip.write_all(&write_npy("<i4", &format!("({},)", lines.len()), &ids))
            .unwrap();
        zip.finish().unwrap();
    }

    fn write_unembedded_segment(dir: &Path, speakers: &[i64]) {
        fs::create_dir_all(dir).unwrap();
        let mut jsonl = String::from("{\"raw\":\"audio.m4a\"}\n");
        for (index, speaker) in speakers.iter().enumerate() {
            jsonl.push_str(&format!(
                "{{\"start\":\"00:00:{index:02}\",\"speaker\":{speaker},\"sentence_id\":{},\"text\":\"line\"}}\n",
                index + 1
            ));
        }
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
    }

    fn write_principal(root: &Path, id: &str, name: &str) {
        write_json(
            &root.join("config/journal.json"),
            json!({"identity":{"name":name,"preferred":name}}),
        );
        write_json(
            &root.join(format!("entities/{id}/entity.json")),
            json!({"id":id,"name":name,"type":"Person","is_principal":true}),
        );
    }

    #[test]
    fn numbered_voice_attendance_threshold_from_activity_read() {
        let cases = [
            (vec![3, 3, 3], "attendee"),
            (vec![3], "mentioned"),
            (vec![3, 3], "attendee"),
        ];
        for (speakers, role) in cases {
            let root = tempfile::tempdir().expect("temporary journal");
            write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
            write_unembedded_segment(
                &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
                &speakers,
            );
            let entry = apply(
                root.path(),
                json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"}),
            );
            assert_eq!(entry["role"], role);
            assert_eq!(entry["entity_id"], Value::Null);
        }
    }

    #[test]
    fn numbered_voice_source_transcript_without_lines_is_mentioned() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));
        write_unembedded_segment(
            &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
            &[1, 1],
        );
        let entry = apply(
            root.path(),
            json!({"name":"Speaker 3","role":"attendee","source":"transcript"}),
        );
        assert_eq!(entry["role"], "mentioned");
        assert_eq!(entry["entity_id"], Value::Null);
    }

    #[test]
    fn numbered_voice_demoted_when_diarization_renumbered_to_other_voice() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_principal(root.path(), "jordan", "Jordan Rivers");
        solstone_core_facets::create_facet(root.path(), FACET, "Work", "", "", "", None).unwrap();
        let chronicle = root.path().join(format!("chronicle/{DAY}"));
        // First segment establishes voice(0, 0.1) as run 1.
        write_embedded_segment(
            &chronicle.join("100000_300"),
            &[(2, voice(0, 0.1)), (1, voice(1, 0.1))],
        );
        let talents1 = chronicle.join("100000_300/talents");
        write_json(
            &talents1.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents1.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents1.join("speakers.json"), json!([]));

        // Later segment in the same run: diarization 3 with voice(0, 0.1) -> renumbered to run 1.
        write_embedded_segment(
            &chronicle.join("100500_300"),
            &[(3, voice(0, 0.1)), (3, voice(0, 0.1)), (3, voice(0, 0.1))],
        );
        let talents2 = chronicle.join("100500_300/talents");
        write_json(
            &talents2.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents2.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents2.join("speakers.json"), json!([]));

        let act = setup_activity(root.path(), &["100500_300"]);
        let entries = apply_for_activity(
            root.path(),
            &act,
            &[json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"})],
        );
        assert_eq!(entries[0]["role"], "mentioned");
        assert_eq!(entries[0]["entity_id"], Value::Null);
    }

    #[test]
    fn numbered_voice_demoted_when_run_named_to_person() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_principal(root.path(), "jordan", "Jordan Rivers");
        solstone_core_facets::create_facet(root.path(), FACET, "Work", "", "", "", None).unwrap();
        write_entity(root.path(), "ada", "Ada Lovelace", "Person");

        let chronicle = root.path().join(format!("chronicle/{DAY}"));
        // First segment: establishes run 1 and run 2
        write_embedded_segment(
            &chronicle.join("100000_300"),
            &[(2, voice(0, 0.1)), (1, voice(1, 0.1))],
        );
        let talents1 = chronicle.join("100000_300/talents");
        write_json(
            &talents1.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents1.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents1.join("speakers.json"), json!([]));

        // Second segment: maintains run 1 and run 2
        write_embedded_segment(
            &chronicle.join("100500_300"),
            &[(1, voice(0, 0.2)), (2, voice(1, 0.0))],
        );
        let talents2 = chronicle.join("100500_300/talents");
        write_json(
            &talents2.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents2.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents2.join("speakers.json"), json!([]));

        // Third segment: new voice(2, 0.0) becomes run 3
        write_embedded_segment(
            &chronicle.join("101000_300"),
            &[(3, voice(2, 0.0)), (3, voice(2, 0.0)), (3, voice(2, 0.0))],
        );
        let talents3 = chronicle.join("101000_300/talents");
        write_json(
            &talents3.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(
            &talents3.join("speaker_labels.json"),
            json!({"labels":[
                {"sentence_id":1,"speaker":"ada","method":"acoustic"},
                {"sentence_id":2,"speaker":"ada","method":"acoustic_cluster"},
                {"sentence_id":3,"speaker":"ada","method":"acoustic"},
            ]}),
        );
        write_json(&talents3.join("speakers.json"), json!([]));

        let act = setup_activity(root.path(), &["101000_300"]);
        let entries = apply_for_activity(
            root.path(),
            &act,
            &[json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"})],
        );
        assert_eq!(entries[0]["role"], "mentioned");
        assert_eq!(entries[0]["entity_id"], Value::Null);
    }

    #[test]
    fn numbered_voice_labelled_to_owner_is_demoted() {
        for method in ["user_assigned", "contextual"] {
            let root = tempfile::tempdir().expect("temporary journal");
            write_principal(root.path(), "jordan", "Jordan Rivers");
            write_participation_fixture(
                root.path(),
                json!({"labels":[
                    {"sentence_id":1,"speaker":"jordan","method":method},
                    {"sentence_id":2,"speaker":"jordan","method":method},
                    {"sentence_id":3,"speaker":"jordan","method":method},
                ]}),
                json!([]),
            );
            write_unembedded_segment(
                &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
                &[3, 3, 3],
            );
            let entry = apply(
                root.path(),
                json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"}),
            );
            assert_eq!(entry["role"], "mentioned", "failed for method {method}");
            assert_eq!(
                entry["entity_id"],
                Value::Null,
                "failed for method {method}"
            );
        }
    }

    #[test]
    fn numbered_voice_ignores_speaker_placeholder_person_entity() {
        let cases = [(vec![3], "mentioned"), (vec![3, 3, 3], "attendee")];
        for (speakers, expected_role) in cases {
            let root = tempfile::tempdir().expect("temporary journal");
            write_participation_fixture(
                root.path(),
                json!({"labels":[
                    {"sentence_id":1,"speaker":"speaker_3","method":"acoustic_cluster"},
                    {"sentence_id":2,"speaker":"speaker_3","method":"acoustic_cluster"},
                ]}),
                json!([]),
            );
            write_entity(root.path(), "speaker_3", "Speaker 3", "Person");
            let detected_path = root
                .path()
                .join(format!("facets/{FACET}/entities/{DAY}.jsonl"));
            let mut detected = fs::read_to_string(&detected_path).unwrap_or_default();
            detected.push_str(&format!(
                "{}\n",
                json!({"id":"speaker_3","name":"Speaker 3","type":"Person"})
            ));
            fs::write(&detected_path, detected).unwrap();

            write_unembedded_segment(
                &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
                &speakers,
            );
            let entry = apply(
                root.path(),
                json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"}),
            );
            assert_eq!(entry["role"], expected_role);
            assert_eq!(entry["entity_id"], Value::Null);
        }
    }

    #[test]
    fn numbered_lines_parity_with_rendered_transcript() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_principal(root.path(), "jordan", "Jordan Rivers");
        solstone_core_facets::create_facet(root.path(), FACET, "Work", "", "", "", None).unwrap();
        write_entity(root.path(), "ada", "Ada Lovelace", "Person");

        let chronicle = root.path().join(format!("chronicle/{DAY}"));
        // Segment 1: embedded run 1 and run 2
        write_embedded_segment(
            &chronicle.join("100000_300"),
            &[(2, voice(0, 0.1)), (1, voice(1, 0.1))],
        );
        let talents1 = chronicle.join("100000_300/talents");
        write_json(
            &talents1.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents1.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents1.join("speakers.json"), json!([]));

        // Segment 2: embedded run 1 (rewritten from 3) and acoustic label to Ada
        write_embedded_segment(
            &chronicle.join("100500_300"),
            &[(3, voice(0, 0.1)), (2, voice(1, 0.0))],
        );
        let talents2 = chronicle.join("100500_300/talents");
        write_json(
            &talents2.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(
            &talents2.join("speaker_labels.json"),
            json!({"labels":[{"sentence_id":2,"speaker":"ada","method":"acoustic"}]}),
        );
        write_json(&talents2.join("speakers.json"), json!([]));

        // Segment 3: unembedded segment whose diarization number 4 is kept
        write_unembedded_segment(&chronicle.join("101000_300"), &[4, 4]);
        let talents3 = chronicle.join("101000_300/talents");
        write_json(
            &talents3.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents3.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents3.join("speakers.json"), json!([]));

        let span = ["100000_300", "100500_300", "101000_300"];
        let lines = collect_numbered_lines(root.path(), DAY, &span, None).unwrap();
        let gate_numbers: BTreeSet<i64> = lines.iter().map(|line| line.number).collect();

        let sources = solstone_core_transcripts::Sources {
            transcripts: true,
            percepts: false,
            talents: solstone_core_transcripts::TalentSource::Disabled,
            voices: crate::transcript::voice_names(root.path()),
        };
        let (screen_transcript, _counts, _memory) =
            solstone_core_transcripts::cluster_span_with_memory(
                root.path(),
                DAY,
                &span,
                &sources,
                None,
            )
            .unwrap();

        let mut rendered_numbers = BTreeSet::new();
        for line in screen_transcript.text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for window in parts.windows(2) {
                if window[0] == "Speaker" && window[1].ends_with(':') {
                    if let Ok(n) = window[1].trim_end_matches(':').parse::<i64>() {
                        rendered_numbers.insert(n);
                    }
                }
            }
        }

        assert_eq!(gate_numbers, rendered_numbers);
        assert!(!gate_numbers.contains(&3));
    }

    #[test]
    fn numbered_attendee_reresolved_when_voice_named() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_principal(root.path(), "jordan", "Jordan Rivers");
        solstone_core_facets::create_facet(root.path(), FACET, "Work", "", "", "", None).unwrap();
        write_entity(root.path(), "bob", "Bob Smith", "Person");

        let act_line = json!({"id":"activity","stream":"_default","segments":[SEGMENT]});
        let act_path = root
            .path()
            .join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        fs::create_dir_all(act_path.parent().unwrap()).unwrap();
        fs::write(&act_path, format!("{act_line}\n")).unwrap();

        let seg_dir = root.path().join(format!("chronicle/{DAY}/{SEGMENT}"));
        let talents = seg_dir.join("talents");
        fs::create_dir_all(&talents).unwrap();
        write_json(
            &talents.join("sense.json"),
            json!({"meeting_detected":true}),
        );
        write_json(&talents.join("speaker_labels.json"), json!({"labels":[]}));
        write_json(&talents.join("speakers.json"), json!([]));
        write_unembedded_segment(&seg_dir, &[3, 3, 3]);

        apply_result(
            root.path(),
            &json!({"participation":[{"name":"Speaker 3","role":"attendee","source":"speaker_label"}]}).to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root.path(), FACET).unwrap(),
            DAY,
            act_line.as_object().unwrap(),
        )
        .unwrap();

        let initial_row: Value = serde_json::from_str(
            fs::read_to_string(&act_path)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(initial_row["participation"][0]["name"], "Speaker 3");
        assert_eq!(initial_row["participation"][0]["role"], "attendee");
        assert_eq!(initial_row["participation"][0]["entity_id"], Value::Null);

        fs::write(seg_dir.join("audio.npz"), b"").unwrap();
        write_json(
            &talents.join("speaker_labels.json"),
            json!({"labels":[
                {"sentence_id":1,"speaker":"bob","method":"acoustic"},
                {"sentence_id":2,"speaker":"bob","method":"acoustic"},
                {"sentence_id":3,"speaker":"bob","method":"acoustic"},
            ]}),
        );

        let count =
            solstone_core_speaker_resolve::activity_reresolution::reresolve_changed_segments(
                root.path(),
                &[
                    solstone_core_speaker_resolve::activity_reresolution::ChangedSegment {
                        day: DAY.to_owned(),
                        stream: "_default".to_owned(),
                        segment_key: SEGMENT.to_owned(),
                    },
                ],
                &["bob".to_owned()],
            )
            .unwrap();
        assert_eq!(count, 1);

        let updated_row: Value = serde_json::from_str(
            fs::read_to_string(&act_path)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(updated_row["participation"][0]["name"], "Bob Smith");
        assert_eq!(updated_row["participation"][0]["entity_id"], "bob");
        assert_eq!(updated_row["participation"][0]["role"], "attendee");
    }

    #[test]
    fn numbered_voice_in_conversation_or_non_meeting_is_never_heard_or_attending() {
        // Arm A: Conversation
        {
            let root = tempfile::tempdir().unwrap();
            write_participation_fixture(root.path(), json!({}), json!([]));
            setup_segment(
                root.path(),
                SEGMENT,
                false,
                Some("social"),
                json!([
                    {"speaker": "person", "method": "acoustic"},
                    {"speaker": "person", "method": "acoustic_cluster"},
                ]),
                &["Good morning, how are things going?", "Working on roadmap."],
                None,
            );
            write_unembedded_segment(
                &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
                &[3, 3, 3],
            );
            let entries = apply_for_activity(
                root.path(),
                &activity(),
                &[
                    json!({"name":"Ada Lovelace","role":"mentioned","source":"transcript"}),
                    json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"}),
                ],
            );
            assert_eq!(entries[0]["name"], "Ada Lovelace");
            assert_eq!(entries[0]["role"], "heard");
            assert_eq!(entries[0]["entity_id"], "person");

            assert_eq!(entries[1]["name"], "Speaker 3");
            assert_eq!(entries[1]["role"], "mentioned");
            assert_eq!(entries[1]["entity_id"], Value::Null);
        }

        // Arm B: Non-meeting, not conversation
        {
            let root = tempfile::tempdir().unwrap();
            write_participation_fixture(root.path(), json!({}), json!([]));
            setup_segment(
                root.path(),
                SEGMENT,
                false,
                Some("coding"),
                json!([]),
                &["fn main() {}"],
                None,
            );
            write_unembedded_segment(
                &root.path().join(format!("chronicle/{DAY}/{SEGMENT}")),
                &[3, 3, 3],
            );
            let entry = apply(
                root.path(),
                json!({"name":"Speaker 3","role":"attendee","source":"speaker_label"}),
            );
            assert_eq!(entry["name"], "Speaker 3");
            assert_eq!(entry["role"], "mentioned");
            assert_eq!(entry["entity_id"], Value::Null);
        }
    }

    #[test]
    fn a_heard_voice_never_names_a_screen_or_mentioned_entry() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_heard_fixture(root.path(), grace_heard_twice());

        for entry in [
            json!({"name":"Grace","role":"mentioned","source":"voice"}),
            json!({"name":"Grace","role":"attendee","source":"screen"}),
        ] {
            let entry = apply(root.path(), entry);
            assert_eq!(entry["entity_id"], Value::Null);
        }
    }

    #[test]
    fn non_speaker_tool_reference_keeps_generic_resolution() {
        let root = tempfile::tempdir().expect("temporary journal");
        write_participation_fixture(root.path(), json!({"labels":[]}), json!([]));

        let entry = apply(
            root.path(),
            json!({"name":"Deploy Bot","role":"attendee","source":"manual"}),
        );

        assert_eq!(entry["entity_id"], "tool");
        assert_eq!(entry["role"], "attendee");
    }
    fn apply_all(root: &Path, entries: Value) -> Vec<(String, String, String)> {
        apply_result(
            root,
            &json!({"participation":entries}).to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root, FACET).unwrap(),
            DAY,
            &activity(),
        )
        .expect("participation applies");
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let activity: Value = serde_json::from_str(
            fs::read_to_string(activity_path)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        activity["participation"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    e["role"].as_str().unwrap().to_owned(),
                    e["source"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn write_sense_entities(root: &Path, meeting: bool, entities: Value) {
        write_json(
            &root.join(format!("chronicle/{DAY}/{SEGMENT}/talents/sense.json")),
            json!({"meeting_detected":meeting,"entities":entities}),
        );
    }

    fn write_screen(root: &Path) {
        fs::write(
            root.join(format!("chronicle/{DAY}/{SEGMENT}/tmux_1_screen.jsonl")),
            "{}\n",
        )
        .unwrap();
    }

    fn row(name: &str, role: &str, source: &str) -> Value {
        json!({"name":name,"role":role,"source":source,"confidence":0.7,"context":"","entity_id":null})
    }

    fn owned(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
        rows.iter()
            .map(|(a, b, c)| ((*a).to_owned(), (*b).to_owned(), (*c).to_owned()))
            .collect()
    }

    #[test]
    fn a_call_recorded_without_audio_lists_its_tiles_as_seen_attendees() {
        // A phone screen recording of a video call with no audio: the call's
        // tiles name Ada, so she attended, but nothing was heard.
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!(["Ada Lovelace"]));
        write_screen(root.path());
        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"speaker_label"}),
        );
        assert_eq!(entry["role"], "attendee");
        assert_eq!(entry["source"], "screen");

        // The same call with its audio keeps the label.
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!(["Ada Lovelace"]));
        write_screen(root.path());
        write_transcript(root.path(), &["Let's start with the roadmap."]);
        let entry = apply(
            root.path(),
            json!({"name":"Ada Lovelace","role":"attendee","source":"speaker_label"}),
        );
        assert_eq!(entry["role"], "attendee");
        assert_eq!(entry["source"], "speaker_label");
    }

    #[test]
    fn names_an_activity_only_showed_on_screen_are_visible() {
        // #404 case 2: authors from an on-screen literature search, listed as mentioned.
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!({}));
        write_screen(root.path());
        // Sense written before `visible` existed, a screen name mislabelled as heard.
        write_sense_entities(
            root.path(),
            false,
            json!([
                {"name":"Cifka","role":"mentioned","source":"voice"},
                {"name":"Suda","role":"visible","source":"screen"}
            ]),
        );
        assert_eq!(
            apply_all(
                root.path(),
                json!([
                    row("Cifka", "mentioned", "voice"),
                    row("Suda", "mentioned", "screen")
                ])
            ),
            owned(&[
                ("Cifka", "visible", "screen"),
                ("Suda", "visible", "screen")
            ])
        );
    }

    #[test]
    fn a_name_that_was_said_stays_mentioned_beside_a_screen() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!({}));
        write_transcript(root.path(), &["Ada said the draft is ready."]);
        write_screen(root.path());
        write_sense_entities(
            root.path(),
            false,
            json!([
                {"name":"Ada Lovelace","role":"mentioned","source":"voice"},
                {"name":"Cifka","role":"visible","source":"screen"}
            ]),
        );
        assert_eq!(
            apply_all(
                root.path(),
                json!([
                    row("Ada Lovelace", "mentioned", "voice"),
                    row("Cifka", "mentioned", "transcript"),
                    row("Grace Hopper", "mentioned", "transcript")
                ])
            ),
            owned(&[
                ("Ada Lovelace", "mentioned", "voice"),
                ("Cifka", "visible", "screen"),
                // Neither said nor seen by Sense: the model's call stands.
                ("Grace Hopper", "mentioned", "transcript"),
            ])
        );
    }

    #[test]
    fn without_a_screen_nothing_is_visible() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!({}));
        write_transcript(root.path(), &["Ada called about the orchard."]);
        write_sense_entities(
            root.path(),
            false,
            json!([{"name":"Atlas","role":"mentioned","source":"screen"}]),
        );
        assert_eq!(
            apply_all(root.path(), json!([row("Atlas", "mentioned", "screen")])),
            owned(&[("Atlas", "mentioned", "screen")])
        );
    }

    fn setup_segment(
        root: &Path,
        segment: &str,
        meeting: bool,
        content_type: Option<&str>,
        labels: Value,
        sentences: &[&str],
        visual_file: Option<&str>,
    ) {
        let talents = root.join(format!("chronicle/{DAY}/{segment}/talents"));
        fs::create_dir_all(&talents).unwrap();
        let mut sense = json!({"meeting_detected": meeting});
        if let Some(ct) = content_type {
            sense["content_type"] = json!(ct);
        }
        write_json(&talents.join("sense.json"), sense);
        write_json(
            &talents.join("speaker_labels.json"),
            json!({"labels": labels}),
        );
        write_json(&talents.join("speakers.json"), json!([]));
        if !sentences.is_empty() {
            let mut text = json!({"raw":"fixture"}).to_string() + "\n";
            for sentence in sentences {
                text += &(json!({"start":"00:00:01","text":sentence}).to_string() + "\n");
            }
            fs::write(
                root.join(format!("chronicle/{DAY}/{segment}/audio.jsonl")),
                text,
            )
            .unwrap();
        }
        if let Some(vf) = visual_file {
            fs::write(root.join(format!("chronicle/{DAY}/{segment}/{vf}")), "{}\n").unwrap();
        }
    }

    fn setup_activity(root: &Path, segments: &[&str]) -> Map<String, Value> {
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        let act = json!({"id":"activity","segments": segments});
        fs::write(activity_path, format!("{act}\n")).unwrap();
        act.as_object().unwrap().clone()
    }

    fn apply_for_activity(root: &Path, act: &Map<String, Value>, entries: &[Value]) -> Vec<Value> {
        apply_result(
            root,
            &json!({"participation": entries}).to_string(),
            FACET,
            &solstone_core_facets::observe_facet_write_identity(root, FACET).unwrap(),
            DAY,
            act,
        )
        .expect("participation applies");
        let act_id = act.get("id").and_then(Value::as_str).unwrap();
        let activity_path = root.join(format!("facets/{FACET}/activities/{DAY}.jsonl"));
        let row: Value = fs::read_to_string(activity_path)
            .expect("activity reads")
            .lines()
            .find_map(|line| {
                let v: Value = serde_json::from_str(line).ok()?;
                (v.get("id").and_then(Value::as_str) == Some(act_id)).then_some(v)
            })
            .expect("activity found");
        row["participation"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn conversation_segment_acoustic_matches_become_heard() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic_cluster"},
            ]),
            &["Good morning, how are things going?", "Working on roadmap."],
            None,
        );
        let entry = apply(
            root.path(),
            json!({
                "name": "Ada Lovelace",
                "role": "mentioned",
                "source": "transcript",
                "confidence": 0.85,
                "context": "Discussed roadmap",
            }),
        );
        assert_eq!(entry["role"], "heard");
        assert_eq!(entry["source"], "voice");
        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["confidence"], 0.85);
        assert_eq!(entry["context"], "Discussed roadmap");
    }

    #[test]
    fn conversation_segment_user_assigned_line_becomes_heard() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([{"speaker": "person", "method": "user_assigned"}]),
            &["Working on roadmap."],
            None,
        );
        let entry = apply(
            root.path(),
            json!({
                "name": "Ada Lovelace",
                "role": "mentioned",
                "source": "transcript",
                "confidence": 0.85,
                "context": "Discussed roadmap",
            }),
        );
        assert_eq!(entry["role"], "heard");
        assert_eq!(entry["source"], "voice");
        assert_eq!(entry["entity_id"], "person");
        assert_eq!(entry["confidence"], 0.85);
        assert_eq!(entry["context"], "Discussed roadmap");
    }

    #[test]
    fn unresolved_name_resolution_and_ambiguity_in_conversation() {
        // 3a: Answering to exactly one conversation-segment voice
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        write_entity(root.path(), "grace", "Grace Hopper", "Person");
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "grace", "method": "acoustic"},
                {"speaker": "grace", "method": "acoustic"},
            ]),
            &["Hello.", "Good to hear from you."],
            None,
        );
        let entry = apply(
            root.path(),
            json!({
                "name": "Grace Hopper",
                "role": "mentioned",
                "source": "transcript",
                "confidence": 0.8,
                "context": "Spoke in convo",
            }),
        );
        assert_eq!(entry["entity_id"], "grace");
        assert_eq!(entry["role"], "heard");
        assert_eq!(entry["source"], "voice");

        // 3b: Name answering only to a voice heard outside conversation segments
        let root2 = tempfile::tempdir().unwrap();
        write_participation_fixture(root2.path(), json!({}), json!([]));
        write_entity(root2.path(), "grace", "Grace Hopper", "Person");
        let act2 = setup_activity(root2.path(), &[SEGMENT, "100000_60"]);
        setup_segment(
            root2.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([]),
            &["Chatting in hallway."],
            None,
        );
        setup_segment(
            root2.path(),
            "100000_60",
            false,
            Some("coding"),
            json!([
                {"speaker": "grace", "method": "acoustic"},
                {"speaker": "grace", "method": "acoustic"},
            ]),
            &["Coding quietly."],
            None,
        );
        let entries2 = apply_for_activity(
            root2.path(),
            &act2,
            &[json!({
                "name": "Grace Hopper",
                "role": "mentioned",
                "source": "transcript",
            })],
        );
        assert_eq!(entries2[0]["entity_id"], Value::Null);
        assert_eq!(entries2[0]["role"], "mentioned");

        // 3c: Two conversation-segment voices answer to the name
        let root3 = tempfile::tempdir().unwrap();
        write_participation_fixture(root3.path(), json!({}), json!([]));
        write_entity(root3.path(), "grace", "Grace Hopper", "Person");
        write_entity(root3.path(), "grace-brewster", "Grace Brewster", "Person");
        setup_segment(
            root3.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "grace", "method": "acoustic"},
                {"speaker": "grace", "method": "acoustic"},
                {"speaker": "grace-brewster", "method": "acoustic"},
                {"speaker": "grace-brewster", "method": "acoustic"},
            ]),
            &["Hello.", "Good to hear from you."],
            None,
        );
        let entry3 = apply(
            root3.path(),
            json!({
                "name": "Grace",
                "role": "mentioned",
                "source": "transcript",
            }),
        );
        assert_eq!(entry3["entity_id"], Value::Null);
        assert_ne!(entry3["role"], "heard");
    }

    #[test]
    fn conversation_segment_voice_wins_over_screen_in_another_segment() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        let act = setup_activity(root.path(), &[SEGMENT, "100000_60"]);
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &["Let us chat about the system.", "Sounds good."],
            None,
        );
        setup_segment(
            root.path(),
            "100000_60",
            false,
            Some("coding"),
            json!([]),
            &[],
            Some("screen.jsonl"),
        );
        let sense2_path = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("100000_60")
            .join("talents/sense.json");
        write_json(
            &sense2_path,
            json!({
                "meeting_detected": false,
                "content_type": "coding",
                "entities": [
                    {"name": "Ada Lovelace", "role": "visible", "source": "screen"}
                ]
            }),
        );
        let entries = apply_for_activity(
            root.path(),
            &act,
            &[json!({
                "name": "Ada Lovelace",
                "role": "mentioned",
                "source": "screen",
            })],
        );
        assert_eq!(entries[0]["role"], "heard");
        assert_eq!(entries[0]["source"], "voice");
        assert_eq!(entries[0]["entity_id"], "person");
    }

    #[test]
    fn non_meeting_conversation_voice_line_thresholds() {
        // 5a: Zero matched lines -> becomes mentioned
        let root1 = tempfile::tempdir().unwrap();
        write_participation_fixture(root1.path(), json!({}), json!([]));
        setup_segment(
            root1.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([]),
            &["Chatting."],
            None,
        );
        let entry1 = apply(
            root1.path(),
            json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"}),
        );
        assert_eq!(entry1["role"], "mentioned");

        // 5b: One acoustic line -> not heard, becomes mentioned
        let root2 = tempfile::tempdir().unwrap();
        write_participation_fixture(root2.path(), json!({}), json!([]));
        setup_segment(
            root2.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([{"speaker": "person", "method": "acoustic"}]),
            &["Chatting."],
            None,
        );
        let entry2 = apply(
            root2.path(),
            json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"}),
        );
        assert_eq!(entry2["role"], "mentioned");

        // 5c: 1 line in conversation segment + 1 line in non-conversation segment -> not heard
        let root3 = tempfile::tempdir().unwrap();
        write_participation_fixture(root3.path(), json!({}), json!([]));
        let act3 = setup_activity(root3.path(), &[SEGMENT, "100000_60"]);
        setup_segment(
            root3.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([{"speaker": "person", "method": "acoustic"}]),
            &["Convo line."],
            None,
        );
        setup_segment(
            root3.path(),
            "100000_60",
            false,
            Some("coding"),
            json!([{"speaker": "person", "method": "acoustic"}]),
            &["Coding line."],
            None,
        );
        let entries3 = apply_for_activity(
            root3.path(),
            &act3,
            &[json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"})],
        );
        assert_eq!(entries3[0]["role"], "mentioned");
    }

    #[test]
    fn conversation_segment_disqualified_by_visual_files_or_empty_speech() {
        // 6a: social with screen.jsonl
        let root1 = tempfile::tempdir().unwrap();
        write_participation_fixture(root1.path(), json!({}), json!([]));
        setup_segment(
            root1.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &["Speaking."],
            Some("screen.jsonl"),
        );
        let entry1 = apply(
            root1.path(),
            json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"}),
        );
        assert_eq!(entry1["role"], "mentioned");

        // 6b: social with browser_pages.jsonl
        let root2 = tempfile::tempdir().unwrap();
        write_participation_fixture(root2.path(), json!({}), json!([]));
        setup_segment(
            root2.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &["Speaking."],
            Some("browser_pages.jsonl"),
        );
        let entry2 = apply(
            root2.path(),
            json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"}),
        );
        assert_eq!(entry2["role"], "mentioned");

        // 6c: social with empty speech (no transcript rows)
        let root3 = tempfile::tempdir().unwrap();
        write_participation_fixture(root3.path(), json!({}), json!([]));
        setup_segment(
            root3.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &[],
            None,
        );
        let entry3 = apply(
            root3.path(),
            json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"}),
        );
        assert_eq!(entry3["role"], "mentioned");
    }

    #[test]
    fn voices_matched_only_in_non_conversation_segment_are_not_heard() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        let act = setup_activity(root.path(), &[SEGMENT, "100000_60"]);
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([]),
            &["Speaking in conversation."],
            None,
        );
        setup_segment(
            root.path(),
            "100000_60",
            false,
            Some("coding"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &["Speaking in coding."],
            None,
        );
        let entries = apply_for_activity(
            root.path(),
            &act,
            &[json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"})],
        );
        assert_eq!(entries[0]["role"], "mentioned");
    }

    #[test]
    fn meeting_segment_in_activity_disables_heard_gate_and_applies_meeting_rules() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        let act = setup_activity(root.path(), &[SEGMENT, "100000_60"]);
        setup_segment(
            root.path(),
            SEGMENT,
            true,
            Some("meeting"),
            json!([]),
            &["Meeting speech."],
            None,
        );
        setup_segment(
            root.path(),
            "100000_60",
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
            ]),
            &["Convo speech."],
            None,
        );
        let entries1 = apply_for_activity(
            root.path(),
            &act,
            &[json!({"name": "Ada Lovelace", "role": "attendee", "source": "voice"})],
        );
        assert_eq!(entries1[0]["role"], "attendee");

        let entries2 = apply_for_activity(
            root.path(),
            &act,
            &[json!({"name": "Ada Lovelace", "role": "mentioned", "source": "transcript"})],
        );
        assert_eq!(entries2[0]["role"], "mentioned");
    }

    #[test]
    fn model_omitted_person_is_not_appended() {
        let root = tempfile::tempdir().unwrap();
        write_participation_fixture(root.path(), json!({}), json!([]));
        write_entity(root.path(), "grace", "Grace Hopper", "Person");
        setup_segment(
            root.path(),
            SEGMENT,
            false,
            Some("social"),
            json!([
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "person", "method": "acoustic"},
                {"speaker": "grace", "method": "acoustic"},
                {"speaker": "grace", "method": "acoustic"},
            ]),
            &["Ada and Grace talking."],
            None,
        );
        let entries = apply_for_activity(
            root.path(),
            &activity(),
            &[json!({
                "name": "Ada Lovelace",
                "role": "mentioned",
                "source": "transcript",
            })],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["entity_id"], "person");
        assert_eq!(entries[0]["role"], "heard");
    }
}
