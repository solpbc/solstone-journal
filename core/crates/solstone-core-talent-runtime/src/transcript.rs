// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_talent_config::{get_output_name, get_talent_filter, source_is_enabled};
use solstone_core_transcripts::{
    ScreenCut, ScreenTranscript, SourceCounts, Sources, TalentSource, cluster,
    cluster_for_screen_talent, cluster_period, cluster_period_for_screen_talent, cluster_span,
    cluster_span_for_screen_talent,
};

pub(crate) struct LoadedTranscript {
    pub text: String,
    pub counts: SourceCounts,
    pub screen_cuts: Vec<ScreenCut>,
}

pub(crate) fn sources_from_config(config: &Map<String, Value>) -> Sources {
    Sources {
        transcripts: config.get("transcripts").is_some_and(source_is_enabled),
        percepts: config.get("percepts").is_some_and(source_is_enabled),
        talents: talent_source(config.get("talents")),
    }
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
    let sources = composed
        .get("sources")
        .and_then(Value::as_object)
        .map(sources_from_config)
        .unwrap_or_else(|| sources_from_config(&Map::new()));
    // Python falls back to SOL_STREAM in talents.py:590-592; this native path has no stream
    // environment seam, so it reads only the composed stream key.
    let stream = composed.get("stream").and_then(Value::as_str);
    let screen_projection = composed.get("name").and_then(Value::as_str) == Some("screen");
    let span = composed
        .get("span")
        .and_then(Value::as_array)
        .map(|span| span.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let (transcript, counts) = if !span.is_empty() {
        if screen_projection {
            cluster_span_for_screen_talent(journal, day, &span, &sources, stream)
        } else {
            cluster_span(journal, day, &span, &sources, stream)
                .map(|(text, counts)| (ScreenTranscript::plain(text), counts))
        }?
    } else if let Some(segment) = composed.get("segment").and_then(Value::as_str) {
        if screen_projection {
            cluster_period_for_screen_talent(journal, day, segment, &sources, stream)
        } else {
            let (text, counts) = cluster_period(journal, day, segment, &sources, stream);
            (ScreenTranscript::plain(text), counts)
        }
    } else if screen_projection {
        cluster_for_screen_talent(journal, day, &sources)
    } else {
        let (text, counts) = cluster(journal, day, &sources);
        (ScreenTranscript::plain(text), counts)
    };
    Ok(LoadedTranscript {
        text: transcript.text,
        counts,
        screen_cuts: transcript.cuts,
    })
}

pub(crate) fn load_segment_transcript(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    config: &Map<String, Value>,
) -> (String, SourceCounts) {
    let sources = sources_from_config(config);
    cluster_period(journal, day, segment, &sources, stream)
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

        assert!(screen.text.contains("**Tmux observation:**"));
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
        assert!(!participation.text.contains("**Tmux observation:**"));
        assert!(!participation.text.contains("## Tmux change encoding"));
        assert!(participation.screen_cuts.is_empty());
    }
}
