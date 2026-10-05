// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_format::content::ConsumedOriginal;
use solstone_core_talent_config::{get_output_name, get_talent_filter, source_is_enabled};
use solstone_core_transcripts::{
    MemoryContext, ScreenCut, SourceCounts, Sources, TalentSource, VoiceNames,
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
}

pub(crate) fn sources_from_config(config: &Map<String, Value>) -> Sources {
    Sources {
        transcripts: config.get("transcripts").is_some_and(source_is_enabled),
        percepts: config.get("percepts").is_some_and(source_is_enabled),
        talents: talent_source(config.get("talents")),
        voices: None,
    }
}

/// The owner and every other admitted person, as transcript lines name them
/// by voice. `None` without a single admitted principal.
pub(crate) fn voice_names(journal: &Path) -> Option<VoiceNames> {
    let principal = crate::JournalOwner::load(journal).ok()?.id?;
    let owner_voice_confirmed = matches!(
        solstone_core_speaker_resolve::owner_centroid::load_owner_centroid(journal, &principal),
        Ok(Some(_))
    );
    let people = solstone_core_entity::load_all_journal_entities(journal)
        .ok()?
        .into_iter()
        .filter(|entity| {
            solstone_core_entity::is_admissible_person(entity) && entity.id != principal
        })
        .filter_map(|entity| {
            let name = entity.value.get("name")?.as_str()?.trim().to_owned();
            (!name.is_empty()).then_some((entity.id, name))
        })
        .collect();
    Some(VoiceNames {
        principal,
        owner_voice_confirmed,
        people,
    })
}

pub(crate) fn sources_are_enabled(config: &Map<String, Value>) -> bool {
    config.values().any(source_is_enabled)
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
}
