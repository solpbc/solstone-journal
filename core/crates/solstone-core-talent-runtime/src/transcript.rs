// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_talent_config::{get_output_name, get_talent_filter, source_is_enabled};
use solstone_core_transcripts::{
    SourceCounts, Sources, TalentSource, cluster, cluster_for_screen_talent, cluster_period,
    cluster_period_for_screen_talent, cluster_span, cluster_span_for_screen_talent,
};

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
) -> Result<(String, SourceCounts), String> {
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
    if !span.is_empty() {
        return if screen_projection {
            cluster_span_for_screen_talent(journal, day, &span, &sources, stream)
        } else {
            cluster_span(journal, day, &span, &sources, stream)
        };
    }
    if let Some(segment) = composed.get("segment").and_then(Value::as_str) {
        return Ok(if screen_projection {
            cluster_period_for_screen_talent(journal, day, segment, &sources, stream)
        } else {
            cluster_period(journal, day, segment, &sources, stream)
        });
    }
    Ok(if screen_projection {
        cluster_for_screen_talent(journal, day, &sources)
    } else {
        cluster(journal, day, &sources)
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

        let screen = load_transcript(journal.path(), &config("screen"))
            .unwrap()
            .0;
        let participation = load_transcript(journal.path(), &config("participation"))
            .unwrap()
            .0;

        assert!(screen.contains("**Tmux observation:**"));
        assert!(screen.contains("## Tmux change encoding"));
        assert!(screen.contains("zero-based `start_line`"));
        assert!(!screen.contains("Terminal session 'main'"));
        assert!(participation.contains("Terminal session 'main'"));
        assert!(!participation.contains("**Tmux observation:**"));
        assert!(!participation.contains("## Tmux change encoding"));
    }
}
