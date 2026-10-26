// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The alternate source packet for activity Work, before provider-side fitting.
//! Native speech/private-memory rendering and missing-result percepts survive.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use crate::transcript;

const REPORT_NOTE: &str = "Existing per-segment model results follow. They are fallible reports, not original observations or independently verified events. Source/speaker labels in these reports do not admit a Voice identity or establish owner authorship. Missing results and fields provide no absence evidence. Native transcripts and raw fallback material retain their original source headings.\n\n";

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub compact_transcript: String,
    pub product_spelling_view: Option<Value>,
    /// Construction provenance; not a claim that the source or reports are whole.
    pub metadata: Value,
}

impl Candidate {
    pub(crate) fn model_transcript(&self) -> &str {
        self.product_spelling_view
            .as_ref()
            .and_then(|view| view.get("text"))
            .and_then(Value::as_str)
            .unwrap_or(&self.compact_transcript)
    }
}

struct Report {
    fields: Map<String, Value>,
    state: &'static str,
    partial_input: Option<Value>,
}

impl Report {
    fn usable(&self) -> bool {
        self.state == "available"
    }

    fn model_value(&self) -> Value {
        let mut value = json!({"availability": self.state, "fields": self.fields});
        if let Some(partial) = &self.partial_input {
            value["partial_input"] = partial.clone();
        }
        value
    }
}

/// Construct only native activity Work's alternate transcript. The caller keeps
/// the original request, protected context and reply allowance, and lets the
/// Generate boundary select this packet only when the complete raw request
/// would not fit. No model call or journal write occurs here.
pub(crate) fn prepare(
    journal: &Path,
    config: &Map<String, Value>,
) -> Result<Option<Candidate>, String> {
    if config.get("name").and_then(Value::as_str) != Some("work")
        || config.get("messages").is_some()
        || !config
            .get("activity")
            .and_then(Value::as_object)
            .is_some_and(|activity| !activity.is_empty())
    {
        return Ok(None);
    }
    // Saved reports may combine speech and percepts. Do not introduce them
    // when an override deliberately excludes either native source family.
    let Some(sources) = config.get("sources").and_then(Value::as_object) else {
        return Ok(None);
    };
    if ["transcripts", "percepts"].iter().any(|source| {
        !sources
            .get(*source)
            .is_some_and(solstone_core_talent_config::source_is_enabled)
    }) {
        return Ok(None);
    }
    let Some(day) = config.get("day").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(span) = config.get("span").and_then(Value::as_array) else {
        return Ok(None);
    };
    if span.is_empty() {
        return Ok(None);
    }
    let stream = config
        .get("stream")
        .and_then(Value::as_str)
        .or_else(|| config.get("activity")?.get("stream")?.as_str())
        .filter(|stream| !stream.is_empty());
    let all_segments = iter_segments(journal, PathOrDay::Day(day))
        .map_err(|error| format!("cannot read Work segment results: {error}"))?;
    let mut segments = Vec::with_capacity(span.len());
    for key in span {
        let Some(key) = key.as_str() else {
            return Ok(None);
        };
        let mut matches = all_segments.iter().filter(|segment| {
            segment.key() == key && stream.is_none_or(|stream| segment.stream().matches(stream))
        });
        let Some(segment) = matches.next() else {
            return Err(format!("Work source segment is unavailable: {day}/{key}"));
        };
        // Without a stream, do not guess which saved report belongs to a key
        // shared by multiple streams. The ordinary raw request remains valid.
        if matches.next().is_some() {
            return Ok(None);
        }
        segments.push(segment);
    }
    segments.sort_by_key(|segment| segment.key());

    let mut compact_transcript = REPORT_NOTE.to_owned();
    let mut compact_sources = Vec::new();
    let mut provenance = Vec::with_capacity(segments.len());
    let mut usable_reports = 0;
    for segment in segments {
        let sense = read_report(&segment.path().join("talents/sense.json"), true);
        let screen = read_report(&segment.path().join("talents/screen.json"), false);
        usable_reports += usize::from(sense.usable()) + usize::from(screen.usable());

        let mut native_config = config.clone();
        native_config.insert("span".into(), json!([segment.key()]));
        native_config.remove("segment");
        // Only a usable Screen narrative replaces native percepts. Sense can
        // still provide summary/topics while a missing Screen keeps raw input.
        if screen.usable()
            && let Some(sources) = native_config
                .get_mut("sources")
                .and_then(Value::as_object_mut)
        {
            sources.insert("percepts".into(), Value::Bool(false));
        }
        let native = transcript::load_transcript(journal, &native_config)?;
        if native.memory_incomplete {
            return Err("private memory context is incomplete".into());
        }

        let reports = json!({"sense": sense.model_value(), "screen": screen.model_value()});
        compact_transcript.push_str(&format!(
            "{}\n\n### Processed segment reports\n{reports}\n\n",
            segment_heading(day, segment.key())?
        ));
        if native.counts.total() > 0 {
            // Includes ordinary Voice naming, imported transcripts and trusted
            // private-memory originals. Do not parse or reconstruct their text.
            let offset = compact_transcript.len();
            compact_transcript.push_str(&native.text);
            compact_sources.extend(native.sources.into_iter().map(|source| {
                source.map(|mut source| {
                    source.rendered_range.start += offset;
                    source.rendered_range.end += offset;
                    for start in &mut source.chunk_offsets {
                        if *start != usize::MAX {
                            *start += offset;
                        }
                    }
                    source
                })
            }));
        }
        compact_transcript.push_str("\n\n");
        provenance.push(json!({
            "segment": segment.key(),
            "sense_availability": sense.state,
            "screen_availability": screen.state,
            "sense_partial_input": sense.partial_input,
            "screen_partial_input": screen.partial_input,
            "raw_percepts_preserved": !screen.usable(),
            "native_transcript_entries": native.counts.transcripts,
            "native_percept_entries": native.counts.percepts,
        }));
    }
    if usable_reports == 0 {
        return Ok(None);
    }
    let sparse_note = config
        .get("transcript")
        .and_then(Value::as_str)
        .filter(|text| text.starts_with(crate::prepare::SPARSE_INPUT_NOTE))
        .map_or("", |_| crate::prepare::SPARSE_INPUT_NOTE);
    compact_transcript.insert_str(0, sparse_note);
    // Reuse the existing source-bound, speech-only model projection. These
    // source offsets exclude the sparse note, as the native projector expects.
    let mut projected = config.clone();
    projected.insert(
        "transcript".into(),
        Value::String(compact_transcript.clone()),
    );
    crate::product_names::prepare("work", &mut projected, &compact_sources);
    Ok(Some(Candidate {
        compact_transcript,
        product_spelling_view: projected.remove(crate::product_names::VIEW_KEY),
        metadata: json!({
            "kind": "processed_segment_reports_with_native_fallback",
            "source_complete": false,
            "native_transcripts_preserved": true,
            "segments": provenance,
        }),
    }))
}

fn segment_heading(day: &str, key: &str) -> Result<String, String> {
    let (clock, length) = key
        .split_once('_')
        .ok_or_else(|| format!("invalid Work source segment: {key}"))?;
    let start = chrono::NaiveDateTime::parse_from_str(&format!("{day} {clock}"), "%Y%m%d %H%M%S")
        .map_err(|error| format!("invalid Work source interval: {error}"))?;
    let seconds = length
        .parse::<i64>()
        .map_err(|error| format!("invalid Work source duration: {error}"))?;
    let duration = chrono::Duration::try_seconds(seconds)
        .ok_or_else(|| "Work source duration is out of range".to_owned())?;
    let end = start
        .checked_add_signed(duration)
        .ok_or_else(|| "Work source interval is out of range".to_owned())?;
    // Use the same observation-entry coordinates the native fitter recognizes.
    Ok(format!(
        "## {} - {}",
        start.format("%Y-%m-%d %H:%M:%S"),
        end.format("%H:%M:%S")
    ))
}

fn read_report(path: &Path, sense: bool) -> Report {
    let unavailable = |state| Report {
        fields: Map::new(),
        state,
        partial_input: None,
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return unavailable("missing");
        }
        Err(error) => {
            log::warn!(
                "cannot read Work segment report {}: {error}",
                path.display()
            );
            return unavailable("unreadable");
        }
    };
    let Ok(Value::Object(value)) = serde_json::from_slice(&bytes) else {
        log::warn!("invalid Work segment report {}", path.display());
        return unavailable("invalid");
    };
    let mut fields = Map::new();
    let text_key = if sense {
        "activity_summary"
    } else {
        "narrative"
    };
    if let Some(text) = value.get(text_key).and_then(Value::as_str)
        && !text.trim().is_empty()
    {
        fields.insert(text_key.into(), Value::String(text.to_owned()));
    }
    if sense
        && let Some(topics) = value.get("topics").and_then(Value::as_array)
        && topics.iter().all(Value::is_string)
    {
        fields.insert("topics".into(), Value::Array(topics.clone()));
    }
    let usable = fields.get(text_key).is_some()
        || fields
            .get("topics")
            .and_then(Value::as_array)
            .is_some_and(|topics| {
                topics
                    .iter()
                    .any(|topic| !topic.as_str().unwrap().trim().is_empty())
            });
    Report {
        fields,
        state: if usable {
            "available"
        } else {
            "empty_or_unusable"
        },
        partial_input: value.get("partial_input").cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn segment(root: &Path, stream: &str, key: &str, marker: &str) -> std::path::PathBuf {
        // Build from components, as journal-io does, so the expected
        // `source_path` matches the product's native separators on Windows.
        let path = root
            .join("chronicle")
            .join("20260101")
            .join(stream)
            .join(key);
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(
            path.join("screen.jsonl"),
            json!({"timestamp": 0, "content": {"window": marker}}).to_string(),
        )
        .unwrap();
        fs::write(
            path.join("audio.jsonl"),
            json!({"start": "00:00:01", "text": format!("Speech {marker}")}).to_string(),
        )
        .unwrap();
        path
    }

    fn config(keys: &[&str]) -> Map<String, Value> {
        json!({"name":"work", "day":"20260101", "span":keys,
            "activity":{"id":"activity", "stream":"desktop"},
            "sources":{"transcripts":true,"percepts":true,"talents":false},
            "transcript":"raw source remains untouched"})
        .as_object()
        .unwrap()
        .clone()
    }

    fn write(path: &Path, name: &str, value: Value) {
        fs::write(path.join("talents").join(name), value.to_string()).unwrap();
    }

    #[test]
    fn reports_are_chronological_and_native_speech_and_missing_percepts_survive() {
        let root = TempDir::new().unwrap();
        let early = segment(root.path(), "desktop", "090000_300", "EARLY_RAW");
        let late = segment(root.path(), "desktop", "091000_300", "LATE_RAW");
        write(
            &early,
            "sense.json",
            json!({"activity_summary":"EARLY_SUMMARY", "topics":["early"], "entities":[{"name":"NOT_ADMITTED"}]}),
        );
        write(
            &early,
            "screen.json",
            json!({"narrative":"EARLY_SCREEN", "entities":[{"name":"NOT_ADMITTED"}], "partial_input":{"dropped_entries":2,"dropped_chars":null}}),
        );
        write(
            &late,
            "sense.json",
            json!({"activity_summary":"LATE_SUMMARY", "topics":["late"]}),
        );
        let config = config(&["091000_300", "090000_300"]);
        let before = config.clone();
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert_eq!(config, before);
        assert!(
            candidate.compact_transcript.find("EARLY_SUMMARY").unwrap()
                < candidate.compact_transcript.find("LATE_SUMMARY").unwrap()
        );
        assert!(!candidate.compact_transcript.contains("NOT_ADMITTED"));
        assert!(candidate.compact_transcript.contains("Speech EARLY_RAW"));
        assert!(candidate.compact_transcript.contains("Speech LATE_RAW"));
        assert!(candidate.compact_transcript.contains("LATE_RAW"));
        assert_eq!(
            candidate.metadata["segments"][0]["screen_partial_input"],
            json!({"dropped_entries":2,"dropped_chars":null})
        );
        assert_eq!(
            candidate.metadata["segments"][0]["native_percept_entries"],
            0
        );
        assert_eq!(
            candidate.metadata["segments"][1]["screen_availability"],
            "missing"
        );
        assert_eq!(
            candidate.metadata["segments"][1]["raw_percepts_preserved"],
            true
        );
        assert_eq!(candidate.metadata["source_complete"], false);
    }

    #[test]
    fn invalid_or_missing_reports_never_remove_raw_units_or_guess_a_stream() {
        let root = TempDir::new().unwrap();
        let own = segment(root.path(), "desktop", "090000_300", "OWN_RAW");
        let other = segment(root.path(), "another", "090000_300", "OTHER_RAW");
        write(&other, "screen.json", json!({"narrative":"OTHER_REPORT"}));
        let mut config = config(&["090000_300"]);
        assert!(prepare(root.path(), &config).unwrap().is_none());
        fs::write(own.join("talents/screen.json"), "incomplete {").unwrap();
        write(&own, "sense.json", json!({"activity_summary":"OWN_REPORT"}));
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(candidate.compact_transcript.contains("OWN_RAW"));
        assert!(!candidate.compact_transcript.contains("OTHER_REPORT"));
        assert_eq!(
            candidate.metadata["segments"][0]["screen_availability"],
            "invalid"
        );
        config["activity"].as_object_mut().unwrap().remove("stream");
        assert!(prepare(root.path(), &config).unwrap().is_none());
    }

    #[test]
    fn only_native_activity_work_gets_an_alternate_and_large_raw_provenance_is_not_required() {
        let root = TempDir::new().unwrap();
        let path = segment(root.path(), "desktop", "090000_300", &"RAW".repeat(25_000));
        write(&path, "screen.json", json!({"narrative":"Compact report"}));
        let mut config = config(&["090000_300"]);
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(candidate.compact_transcript.contains("Compact report"));
        assert!(candidate.compact_transcript.contains("Speech RAW"));
        for name in ["screen", "sense", "conversation", "participation"] {
            config.insert("name".into(), json!(name));
            assert!(prepare(root.path(), &config).unwrap().is_none());
        }
        config.insert("name".into(), json!("work"));
        config.insert(
            "messages".into(),
            json!([{"role":"user","content":"explicit"}]),
        );
        assert!(prepare(root.path(), &config).unwrap().is_none());
        config.remove("messages");
        for source in ["transcripts", "percepts"] {
            config["sources"][source] = json!(false);
            assert!(prepare(root.path(), &config).unwrap().is_none());
            config["sources"][source] = json!(true);
        }
        config.remove("activity");
        assert!(prepare(root.path(), &config).unwrap().is_none());
    }

    #[test]
    fn compact_speech_keeps_native_voice_gates_and_the_source_bound_spelling_view() {
        let root = TempDir::new().unwrap();
        let path = segment(root.path(), "desktop", "090000_300", "DISPLAY");
        for (id, name, principal) in [("owner", "Avery", true), ("person", "Mina", false)] {
            let dir = root.path().join("entities").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("entity.json"),
                json!({"id":id,"name":name,"type":"Person","is_principal":principal}).to_string(),
            )
            .unwrap();
        }
        fs::write(path.join("audio.jsonl"), [
            json!({"start":"00:00:01","sentence_id":1,"speaker":1,"text":"Discuss Soulstone with me."}).to_string(),
            json!({"start":"00:00:02","sentence_id":2,"speaker":2,"text":"Unconfirmed identity."}).to_string(),
            json!({"start":"00:00:03","sentence_id":3,"speaker":2,"text":"Heard by voice."}).to_string(),
        ].join("\n")).unwrap();
        write(
            &path,
            "speaker_labels.json",
            json!({"labels":[
                {"sentence_id":1,"speaker":"owner","method":"user_confirmed"},
                {"sentence_id":2,"speaker":"person","method":"contextual"},
                {"sentence_id":3,"speaker":"person","method":"acoustic"}
            ]}),
        );
        write(
            &path,
            "screen.json",
            json!({"narrative":"Écran says Soulstone literally."}),
        );
        let mut config = config(&["090000_300"]);
        let original = transcript::load_transcript(root.path(), &config).unwrap();
        config.insert(
            "transcript".into(),
            json!(format!(
                "{}{}",
                crate::prepare::SPARSE_INPUT_NOTE,
                original.text
            )),
        );
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(
            candidate
                .compact_transcript
                .starts_with(crate::prepare::SPARSE_INPUT_NOTE)
        );
        assert!(
            candidate
                .compact_transcript
                .contains("You: Discuss Soulstone with me.")
        );
        assert!(
            candidate
                .model_transcript()
                .contains("You: Discuss solstone with me.")
        );
        assert!(
            candidate
                .model_transcript()
                .contains("Speaker 2: Unconfirmed identity.")
        );
        assert!(
            candidate
                .model_transcript()
                .contains("Mina: Heard by voice.")
        );
        // Saved Screen prose is never swept into speech's spelling projection.
        assert!(
            candidate
                .model_transcript()
                .contains("Écran says Soulstone literally.")
        );
        let view = candidate.product_spelling_view.as_ref().unwrap();
        let line = view["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|line| !line["edits"].as_array().unwrap().is_empty())
            .unwrap();
        let at = line["original_start"].as_u64().unwrap() as usize;
        assert!(candidate.compact_transcript[at..].starts_with("Discuss Soulstone with me."));
        assert_eq!(
            line["source_path"],
            path.join("audio.jsonl").display().to_string()
        );
    }
}
