// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::Path;

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_entity::normalize_embedding;
use solstone_core_format::content::ConsumedOriginal;
use solstone_core_journal_io::SegmentLayout;
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use solstone_core_speaker_resolve::owner_centroid::load_owner_centroid;
use solstone_core_speaker_resolve::resolve::{ResolveOutcome, resolve};
use solstone_core_speaker_resolve::segment_catalog::resolve_exact_dir;
use solstone_core_talent_config::{get_output_name, get_talent_filter, source_is_enabled};
use solstone_core_transcripts::{
    EntrySource, MemoryContext, ScreenCut, SourceCounts, Sources, TalentSource, VoiceNames,
    cluster_for_screen_talent_with_memory, cluster_period_for_screen_talent_with_memory,
    cluster_period_with_memory, cluster_span_for_screen_talent_with_memory,
    cluster_span_with_memory, cluster_with_memory,
};

pub(crate) struct LoadedTranscript {
    pub text: String,
    pub counts: SourceCounts,
    pub screen_cuts: Vec<ScreenCut>,
    pub memory_sources: Vec<ConsumedOriginal>,
    pub memory_incomplete: bool,
    pub sources: Vec<Option<EntrySource>>,
}

pub(crate) fn sources_from_config(config: &Map<String, Value>) -> Sources {
    Sources {
        transcripts: config.get("transcripts").is_some_and(source_is_enabled),
        percepts: config.get("percepts").is_some_and(source_is_enabled),
        talents: talent_source(config.get("talents")),
        voices: None,
        heard_owner: None,
        // A talent never reads a workout: it is body data.
        strava_tiles: false,
    }
}

/// The owner and every other admitted person, as transcript lines name them
/// by voice. `None` without a single admitted principal.
pub fn voice_names(journal: &Path) -> Option<VoiceNames> {
    let entities = solstone_core_entity::load_all_journal_entities(journal).ok()?;
    let owner = crate::JournalOwner::from_entities(journal, &entities);
    voice_names_from_entities(journal, &owner, &entities)
}

pub(crate) fn voice_names_from_entities(
    journal: &Path,
    owner: &crate::JournalOwner,
    entities: &[solstone_core_entity::JournalEntity],
) -> Option<VoiceNames> {
    let principal = owner.id.clone()?;
    let owner_voice_confirmed = matches!(
        // `owner` admitted this principal from these entities.
        solstone_core_speaker_resolve::owner_centroid::load_centroid_of_admitted_owner(
            journal, &principal,
        ),
        Ok(Some(_))
    );
    let people = entities
        .iter()
        .filter(|entity| {
            solstone_core_entity::is_admissible_person(entity) && entity.id != principal
        })
        .filter_map(|entity| {
            let name = entity.value.get("name")?.as_str()?.trim().to_owned();
            (!name.is_empty()).then_some((entity.id.clone(), name))
        })
        .collect();
    Some(VoiceNames {
        principal,
        owner_voice_confirmed,
        people,
        owner_name: None,
    })
}

/// Who an agent's transcript read names: [`voice_names`], with the owner's own
/// recognized lines named "<owner name> (owner)" so an agent can tell them from
/// everyone else's. The same names read in `voices`. `None` without a single
/// admitted principal.
pub fn agent_speakers(journal: &Path) -> Option<solstone_core_transcripts::AgentSpeakers> {
    let entities = solstone_core_entity::load_all_journal_entities(journal).ok()?;
    let owner = crate::JournalOwner::from_entities(journal, &entities);
    let mut voices = voice_names_from_entities(journal, &owner, &entities)?;
    let name = entities
        .into_iter()
        .find(|entity| entity.id == voices.principal)
        .and_then(|entity| Some(entity.value.get("name")?.as_str()?.trim().to_owned()))
        .filter(|name| !name.is_empty());
    let owner = name.map_or_else(|| "The owner".to_owned(), |name| format!("{name} (owner)"));
    voices.owner_name = Some(owner.clone());
    Some(solstone_core_transcripts::AgentSpeakers { owner, voices })
}

pub(crate) fn sources_are_enabled(config: &Map<String, Value>) -> bool {
    config.values().any(source_is_enabled)
}

/// How closely a line of an imported recording must match the owner's voiceprint
/// to read You. Live capture names the owner from 0.43, but a recording can be
/// anyone's: strangers' lines scored up to 0.47 against one owner's voiceprint,
/// and that owner's own matched lines a median of 0.69.
const IMPORTED_OWNER_SCORE_MIN: f32 = 0.5;

/// The lines of an imported recording heard as the owner's voice: the owner
/// voiceprint's own match, never one its margin declined, at the import bar.
/// Read-only. `None` outside an `import.*` segment with voice embeddings, or
/// when none are heard.
fn imported_owner_lines(journal: &Path, composed: &Map<String, Value>) -> Option<BTreeSet<i64>> {
    let stream = composed
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| stream.starts_with("import."))?;
    let day = composed.get("day").and_then(Value::as_str)?;
    let segment = composed.get("segment").and_then(Value::as_str)?;
    if day.is_empty() || segment.is_empty() {
        return None;
    }
    let layout = match composed.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => SegmentLayout::Direct,
        _ => SegmentLayout::Named,
    };
    let ResolveOutcome::Resolved(resolved) = resolve(
        journal,
        day,
        stream,
        segment,
        layout,
        true,
        Utc::now().timestamp_millis(),
    )
    .ok()?
    else {
        return None;
    };
    let matched = resolved
        .labels
        .iter()
        .filter(|label| {
            label.method.as_deref() == Some("owner_centroid")
                && label.owner_margin_declined != Some(true)
        })
        .filter_map(|label| Some((label.sentence_id, label.speaker.as_deref()?)))
        .collect::<Vec<_>>();
    let owner_id = matched.first()?.1;
    let owner = load_owner_centroid(journal, owner_id).ok()??;
    let source = resolved.source.as_deref()?;
    let dir = resolve_exact_dir(journal, day, stream, segment, layout).ok()?;
    let embeddings = load_embeddings_file(&dir.join(format!("{source}.npz"))).ok()??;
    let score = |sentence_id: i64| {
        embeddings
            .statements
            .iter()
            .find(|(id, _)| *id == sentence_id)
            .and_then(|(_, embedding)| normalize_embedding(embedding))
            .map(|embedding| {
                embedding
                    .iter()
                    .zip(&owner.centroid)
                    .map(|(left, right)| left * right)
                    .sum::<f32>()
            })
    };
    let heard = matched
        .iter()
        .filter(|(sentence_id, _)| score(*sentence_id) >= Some(IMPORTED_OWNER_SCORE_MIN))
        .map(|(sentence_id, _)| *sentence_id)
        .collect::<BTreeSet<_>>();
    // A recording with one voice is the owner's or it isn't. A stranger's voice
    // can match a few of its lines, so the owner is heard there only when most
    // of them match. Several voices keep the per-line match.
    let one_voice = speaker_evidence(&dir, source).as_deref() != Some("multi");
    if heard.is_empty() || (one_voice && heard.len() * 2 < resolved.labels.len()) {
        return None;
    }
    Some(heard)
}

/// The `speaker_evidence` an audio transcript's header records.
fn speaker_evidence(dir: &Path, source: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(format!("{source}.jsonl"))).ok()?;
    let header = serde_json::from_str::<Value>(text.lines().next()?).ok()?;
    Some(header.get("speaker_evidence")?.as_str()?.to_owned())
}

pub(crate) fn load_transcript(
    journal: &Path,
    composed: &Map<String, Value>,
) -> Result<LoadedTranscript, String> {
    let day = composed
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut sources = composed
        .get("sources")
        .and_then(Value::as_object)
        .map(sources_from_config)
        .unwrap_or_else(|| sources_from_config(&Map::new()));
    // An activity's talents read who spoke by voice; segment talents, which
    // produce the labels, keep the anonymous diarization speakers.
    if composed
        .get("activity")
        .and_then(Value::as_object)
        .is_some_and(|activity| !activity.is_empty())
    {
        sources.voices = voice_names(journal);
    }
    // Sense reads an imported recording before its speakers are labelled, and
    // its import rule gives the owner only the lines labelled You. Hearing the
    // owner's voice needs no model, so those lines read You here. Live capture
    // keeps the anonymous numbers it labels from.
    if composed.get("name").and_then(Value::as_str) == Some("sense") {
        sources.heard_owner = imported_owner_lines(journal, composed);
    }
    // An activity's segments are read from the stream its record names, when
    // the request names none: another stream can hold the same segment key.
    let stream = composed.get("stream").and_then(Value::as_str).or_else(|| {
        composed
            .get("activity")
            .and_then(|activity| activity.get("stream"))
            .and_then(Value::as_str)
    });
    let screen_projection = composed.get("name").and_then(Value::as_str) == Some("screen");
    let span = composed
        .get("span")
        .and_then(Value::as_array)
        .map(|span| span.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let (transcript, counts, memory) = if !span.is_empty() {
        if screen_projection {
            cluster_span_for_screen_talent_with_memory(journal, day, &span, &sources, stream)
        } else {
            cluster_span_with_memory(journal, day, &span, &sources, stream)
        }?
    } else if let Some(segment) = composed.get("segment").and_then(Value::as_str) {
        if screen_projection {
            cluster_period_for_screen_talent_with_memory(journal, day, segment, &sources, stream)
        } else if composed.get("name").and_then(Value::as_str) == Some("sense") {
            solstone_core_transcripts::cluster_period_for_terminal_sense_with_memory(
                journal, day, segment, &sources, stream,
            )
        } else {
            cluster_period_with_memory(journal, day, segment, &sources, stream)
        }
    } else if screen_projection {
        cluster_for_screen_talent_with_memory(journal, day, &sources)
    } else {
        cluster_with_memory(journal, day, &sources)
    };
    let MemoryContext {
        sources: memory_sources,
        incomplete: memory_incomplete,
    } = memory;
    Ok(LoadedTranscript {
        text: transcript.text,
        counts,
        screen_cuts: transcript.cuts,
        memory_sources,
        memory_incomplete,
        sources: transcript.sources,
    })
}

pub(crate) fn load_segment_transcript(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    config: &Map<String, Value>,
) -> (String, SourceCounts, MemoryContext) {
    let sources = sources_from_config(config);
    let (transcript, counts, memory) =
        cluster_period_with_memory(journal, day, segment, &sources, stream);
    (transcript.text, counts, memory)
}

/// Render verbatim transcript text for an activity's segments without LLM generation.
pub fn activity_span_transcript(
    journal: &Path,
    day: &str,
    span: &[&str],
    stream: Option<&str>,
) -> Result<String, String> {
    let sources = Sources {
        transcripts: true,
        percepts: false,
        talents: TalentSource::Disabled,
        voices: None,
        heard_owner: None,
        strava_tiles: false,
    };
    let (transcript, _counts, _memory) =
        cluster_span_with_memory(journal, day, span, &sources, stream)?;
    Ok(transcript.text)
}

fn talent_source(value: Option<&Value>) -> TalentSource {
    let Some(value) = value else {
        return TalentSource::Disabled;
    };
    match get_talent_filter(value) {
        None if source_is_enabled(value) => TalentSource::All,
        None => TalentSource::Disabled,
        Some(filter) if filter.is_empty() => TalentSource::Disabled,
        Some(filter) => {
            let stems = filter
                .iter()
                .filter(|(_, value)| {
                    matches!(value, Value::Bool(true)) || value.as_str() == Some("required")
                })
                .map(|(key, _)| get_output_name(key))
                .collect::<BTreeSet<_>>();
            if stems.is_empty() {
                TalentSource::Disabled
            } else {
                TalentSource::Only(stems)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn person(journal: &Path, id: &str, value: serde_json::Value) {
        let dir = journal.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"), value.to_string()).unwrap();
    }

    #[test]
    fn agent_speakers_mark_the_owner_by_name_and_need_one_principal() {
        let journal = TempDir::new().unwrap();
        assert!(agent_speakers(journal.path()).is_none());
        person(
            journal.path(),
            "mina",
            json!({"id": "mina", "name": "Mina", "type": "Person"}),
        );
        assert!(agent_speakers(journal.path()).is_none());
        person(
            journal.path(),
            "olu",
            json!({"id": "olu", "name": "Olu", "type": "Person", "is_principal": true}),
        );
        let speakers = agent_speakers(journal.path()).unwrap();
        assert_eq!(speakers.owner, "Olu (owner)");
        assert_eq!(speakers.voices.owner_label(), "Olu (owner)");
        assert_eq!(speakers.voices.principal, "olu");
        assert_eq!(
            speakers.voices.people.get("mina").map(String::as_str),
            Some("Mina")
        );
        // Two principals is a damaged identity: no one is named.
        person(
            journal.path(),
            "ada",
            json!({"id": "ada", "name": "Ada", "type": "Person", "is_principal": true}),
        );
        assert!(agent_speakers(journal.path()).is_none());
    }

    /// A day talent's load, and a segment load such as the entity suggestion's,
    /// hold no Strava workout: it is body data. A capture beside it still reads.
    #[test]
    fn a_talent_day_load_holds_no_strava_workout_text() {
        let journal = TempDir::new().unwrap();
        let day = journal.path().join("chronicle/20260903");
        let tile = day.join("import.strava/070000_300");
        fs::create_dir_all(&tile).unwrap();
        fs::write(
            tile.join("workout.json"),
            json!({
                "schema": "solstone.import.strava.tile.v1",
                "activity_id": 9,
                "tile": {"index": 0, "count": 1},
                "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0,
                    "strava_fields": {"Activity Gear": "Trek Domane SL6 (Blue Thunder)"}}
            })
            .to_string(),
        )
        .unwrap();
        let capture = day.join("default/090000_300");
        fs::create_dir_all(&capture).unwrap();
        fs::write(
            capture.join("audio.jsonl"),
            r#"{"start":"00:00:01","text":"garden planning"}"#,
        )
        .unwrap();
        let sources = json!({"transcripts": true, "percepts": true, "talents": true});
        let loaded = load_transcript(
            journal.path(),
            json!({"day": "20260903", "sources": sources})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert!(loaded.text.contains("garden planning"), "{}", loaded.text);
        assert!(!loaded.text.contains("Trek"), "{}", loaded.text);
        assert!(!loaded.text.contains("Morning Run"), "{}", loaded.text);
        let (segment, _, _) = load_segment_transcript(
            journal.path(),
            "20260903",
            "070000_300",
            Some("import.strava"),
            sources.as_object().unwrap(),
        );
        assert!(!segment.contains("Morning Run"), "{segment}");
        // The same tile read the way the owner's own page reads it shows its
        // gear, so the absence above is the talent's load leaving it out.
        let owner = Sources {
            strava_tiles: true,
            ..sources_from_config(sources.as_object().unwrap())
        };
        let (page, _) = solstone_core_transcripts::cluster(journal.path(), "20260903", &owner);
        assert!(page.contains("Trek"), "{page}");
        assert!(page.contains("Morning Run"), "{page}");
    }

    #[test]
    fn an_activity_reads_its_own_streams_segment_when_another_stream_shares_the_key() {
        let journal = TempDir::new().unwrap();
        let day = journal.path().join("chronicle/20260903");
        let own = day.join("tmux/102159_300");
        fs::create_dir_all(&own).unwrap();
        fs::write(
            own.join("tmux_0_screen.jsonl"),
            include_str!(
                "../../solstone-core-format/tests/data/golden/tmux-observer-envelope-main.jsonl"
            ),
        )
        .unwrap();
        // Sorts first, so a lookup by key alone lands here.
        fs::create_dir_all(day.join("desktop/102159_300")).unwrap();
        let request = json!({
            "name": "participation",
            "day": "20260903",
            "span": ["102159_300"],
            "activity": {"id": "terminal_102159_300", "stream": "tmux"},
            "sources": {"percepts": true}
        });

        let loaded = load_transcript(journal.path(), request.as_object().unwrap()).unwrap();

        assert!(
            loaded.text.contains("Terminal session 'main'"),
            "{}",
            loaded.text
        );
    }

    #[test]
    fn only_screen_selects_the_tmux_talent_projection() {
        let journal = TempDir::new().unwrap();
        let segment = journal.path().join("chronicle/20260903/device/102159_300");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("tmux_0_screen.jsonl"),
            include_str!(
                "../../solstone-core-format/tests/data/golden/tmux-observer-envelope-main.jsonl"
            ),
        )
        .unwrap();
        let config = |name| {
            json!({
                "name": name,
                "day": "20260903",
                "segment": "102159_300",
                "stream": "device",
                "sources": {"percepts": true}
            })
            .as_object()
            .unwrap()
            .clone()
        };

        let screen = load_transcript(journal.path(), &config("screen")).unwrap();
        let participation = load_transcript(journal.path(), &config("participation")).unwrap();

        assert!(screen.text.contains("**Tmux window:**"));
        assert!(screen.text.contains("## Tmux change encoding"));
        assert!(screen.text.contains("zero-based `start_line`"));
        assert!(!screen.text.contains("Terminal session 'main'"));
        assert_eq!(screen.screen_cuts.len(), 1);
        let cut = screen.screen_cuts[0];
        assert!(cut.reset_carry);
        assert!(screen.text.is_char_boundary(cut.byte_offset));
        assert!(screen.text.is_char_boundary(cut.observation_byte_offset));
        assert!(screen.text[cut.byte_offset..].starts_with("### Screen Activity"));
        assert!(screen.text[cut.observation_byte_offset..].starts_with("### 10:21:59"));

        assert!(participation.text.contains("Terminal session 'main'"));
        assert!(!participation.text.contains("**Tmux window:**"));
        assert!(!participation.text.contains("## Tmux change encoding"));
        assert!(participation.screen_cuts.is_empty());
    }

    #[test]
    fn participation_transcript_assembly_ignores_topics_md_and_sense_topics_quotes() {
        let journal = TempDir::new().unwrap();
        let segment = journal.path().join("chronicle/20260903/device/102159_300");
        let talents_dir = segment.join("talents");
        fs::create_dir_all(&talents_dir).unwrap();

        fs::write(talents_dir.join("shape.json"), r#"{"sense.json":"Sense"}"#).unwrap();

        let sense_base = json!({
            "density": "steady",
            "content_type": "meeting",
            "activity_summary": "Original discussion summary.",
            "entities": [],
            "facets": [{"facet": "work", "activity": "Discussion", "level": "high"}],
            "speculative_facet": null,
            "meeting_detected": false,
            "speakers": [],
            "recommend": {"screen_record": false, "speaker_attribution": false}
        });
        fs::write(
            talents_dir.join("sense.json"),
            serde_json::to_string_pretty(&sense_base).unwrap(),
        )
        .unwrap();

        let request = json!({
            "name": "participation",
            "day": "20260903",
            "segment": "102159_300",
            "span": ["102159_300"],
            "stream": "device",
            "activity": {},
            "sources": {
                "talents": {
                    "sense": true
                }
            }
        });

        let loaded_base = load_transcript(journal.path(), request.as_object().unwrap()).unwrap();
        assert!(
            loaded_base.text.contains("Original discussion summary."),
            "base transcript should contain activity summary: {}",
            loaded_base.text
        );

        // 1. Adding talents/topics.md does not alter participation transcript text
        fs::write(
            talents_dir.join("topics.md"),
            "## Topics\n- budget review\n\n## Quotes\n- \"budget numbers\"\n",
        )
        .unwrap();
        let loaded_with_topics_md =
            load_transcript(journal.path(), request.as_object().unwrap()).unwrap();
        assert_eq!(
            loaded_base.text, loaded_with_topics_md.text,
            "topics.md must not affect participation transcript assembly"
        );

        // 2. Adding topics and quotes to sense.json does not alter participation transcript text
        let mut sense_with_keys = sense_base.clone();
        sense_with_keys["topics"] = json!(["budget review"]);
        sense_with_keys["quotes"] = json!([{"speaker": "Alice", "text": "budget numbers"}]);
        fs::write(
            talents_dir.join("sense.json"),
            serde_json::to_string_pretty(&sense_with_keys).unwrap(),
        )
        .unwrap();
        let loaded_with_keys =
            load_transcript(journal.path(), request.as_object().unwrap()).unwrap();
        assert_eq!(
            loaded_base.text, loaded_with_keys.text,
            "sense.json topics and quotes must not affect participation transcript assembly"
        );

        // Positive control: changing activity_summary does alter transcript text
        let mut sense_changed = sense_with_keys.clone();
        sense_changed["activity_summary"] = json!("Updated discussion summary.");
        fs::write(
            talents_dir.join("sense.json"),
            serde_json::to_string_pretty(&sense_changed).unwrap(),
        )
        .unwrap();
        let loaded_changed = load_transcript(journal.path(), request.as_object().unwrap()).unwrap();
        assert_ne!(
            loaded_base.text, loaded_changed.text,
            "activity_summary modification should change transcript text"
        );
        assert!(
            loaded_changed.text.contains("Updated discussion summary."),
            "changed transcript should reflect new activity summary"
        );
    }

    fn heard_owner_journal(stream: &str, evidence: &str, owner_lines: usize) -> TempDir {
        heard_owner_journal_at(stream, evidence, owner_lines, 1.0)
    }

    /// Four lines: the first `owner_lines` score `owner_score` against the
    /// owner's voiceprint, the rest are a voice orthogonal to it.
    fn heard_owner_journal_at(
        stream: &str,
        evidence: &str,
        owner_lines: usize,
        owner_score: f32,
    ) -> TempDir {
        let journal = TempDir::new().unwrap();
        person(
            journal.path(),
            "owner",
            json!({"id": "owner", "name": "Owner", "type": "Person", "is_principal": true}),
        );
        let mut owner = vec![0.0; 256];
        owner[0] = 1.0;
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            journal.path(),
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: owner.clone(),
                cluster_size: 10,
                timestamp: "2026-08-08T12:00:00Z".to_owned(),
                evidence_tier: "high".to_owned(),
            },
        )
        .unwrap();
        let mut stranger = vec![0.0; 256];
        stranger[1] = 1.0;
        let segment = journal
            .path()
            .join("chronicle/20260808")
            .join(stream)
            .join("120000_300");
        fs::create_dir_all(&segment).unwrap();
        let ids = [1, 2, 3, 4];
        let values = (0..4)
            .map(|index| {
                if index < owner_lines {
                    let mut line = vec![0.0; 256];
                    line[0] = owner_score;
                    line[2] = (1.0 - owner_score * owner_score).sqrt();
                    line
                } else {
                    stranger.clone()
                }
            })
            .collect::<Vec<_>>();
        crate::speaker_attribution::tests::write_statement_embeddings(
            &segment.join("imported_audio.npz"),
            &ids,
            &values,
        );
        let mut lines =
            vec![json!({"raw": "imported_audio.m4a", "speaker_evidence": evidence}).to_string()];
        lines.extend(ids.iter().map(|id| {
            json!({"start": "12:00:0{id}", "sentence_id": id, "text": "words"}).to_string()
        }));
        fs::write(segment.join("imported_audio.jsonl"), lines.join("\n")).unwrap();
        journal
    }

    fn heard(journal: &TempDir, stream: &str) -> Option<BTreeSet<i64>> {
        let composed =
            json!({"name": "sense", "day": "20260808", "segment": "120000_300", "stream": stream});
        imported_owner_lines(journal.path(), composed.as_object().unwrap())
    }

    #[test]
    fn an_imported_recording_hears_the_owner_only_where_one_voice_is_mostly_theirs() {
        // One voice: most lines the owner's is the owner's recording.
        let journal = heard_owner_journal("import.audio", "single", 3);
        assert_eq!(heard(&journal, "import.audio"), Some([1, 2, 3].into()));
        // One voice with a few owner-like lines is someone else's.
        let journal = heard_owner_journal("import.audio", "single", 1);
        assert_eq!(heard(&journal, "import.audio"), None);
        // Several voices keep the per-line match.
        let journal = heard_owner_journal("import.audio", "multi", 1);
        assert_eq!(heard(&journal, "import.audio"), Some([1].into()));
        // A line that clears the live bar but not the import bar is not heard.
        let journal = heard_owner_journal_at("import.audio", "multi", 1, 0.46);
        assert_eq!(heard(&journal, "import.audio"), None);
        // Live capture keeps its anonymous numbers.
        let journal = heard_owner_journal("mic", "single", 4);
        assert_eq!(heard(&journal, "mic"), None);
    }
}
