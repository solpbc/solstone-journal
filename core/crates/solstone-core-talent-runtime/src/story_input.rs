// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The alternate source packet for the activity Story talents (Work and
//! Conversation), fitted to the floor window by design.
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

    fn narrative_chars(&self) -> usize {
        self.fields
            .get("narrative")
            .and_then(Value::as_str)
            .map_or(0, |text| text.chars().count())
    }

    fn model_value_capped(&self, cap: Option<usize>) -> Value {
        let mut value = self.model_value();
        if let Some(cap) = cap
            && let Some(text) = self.fields.get("narrative").and_then(Value::as_str)
            && text.chars().count() > cap
        {
            value["fields"]["narrative"] = Value::String(cut_at_sentence(text, cap));
            value["narrative_cut_to_fit"] = Value::Bool(true);
        }
        value
    }

    fn model_value(&self) -> Value {
        let mut value = json!({"availability": self.state, "fields": self.fields});
        if let Some(partial) = &self.partial_input {
            value["partial_input"] = partial.clone();
        }
        value
    }
}

/// Construct the Story talents' (Work, Conversation) alternate transcript,
/// fitted to the floor window. The caller keeps the original request,
/// protected context and reply allowance, and makes this packet the input when
/// the complete raw request would not fit the floor (`over_floor`), on every
/// backend. No model call or journal write occurs here.
pub(crate) fn prepare(
    journal: &Path,
    config: &Map<String, Value>,
) -> Result<Option<Candidate>, String> {
    if !matches!(
        config.get("name").and_then(Value::as_str),
        Some("work" | "conversation")
    ) || config.get("messages").is_some()
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

    let mut parts = Vec::with_capacity(segments.len());
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
        parts.push(Part {
            heading: segment_heading(day, segment.key())?,
            sense,
            screen,
            // Includes ordinary Voice naming, imported transcripts and trusted
            // private-memory originals. Do not parse or reconstruct their text.
            native: (native.counts.total() > 0).then_some((native.text, native.sources)),
        });
    }
    if usable_reports == 0 {
        return Ok(None);
    }
    let fit = fit_to_floor(config, &parts);
    let (compact_transcript, compact_sources) = assemble(&parts, &fit.keep, fit.narrative_cap);
    for (entry, kept) in provenance.iter_mut().zip(&fit.keep) {
        entry["native_shown"] = json!(*kept);
    }
    let mut compact_transcript = compact_transcript;
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
    let name = config
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    crate::product_names::prepare(name, &mut projected, &compact_sources);
    Ok(Some(Candidate {
        compact_transcript,
        product_spelling_view: projected.remove(crate::product_names::VIEW_KEY),
        metadata: json!({
            "kind": "processed_segment_reports_with_native_fallback",
            "source_complete": false,
            "native_transcripts_preserved": fit.keep.iter().all(|kept| *kept),
            "screen_narrative_cap_chars": fit.narrative_cap,
            "segments": provenance,
        }),
    }))
}

/// Shown where a segment's own text is left out to fit the floor window.
const DROPPED_NOTE: &str =
    "(This segment's own speech and screen text are left out to fit; its report above tells it.)";
/// Room kept beside the instruction for message framing.
const FRAMING_TOKENS: u32 = 256;

type Native = (String, Vec<Option<solstone_core_transcripts::EntrySource>>);

struct Part {
    heading: String,
    sense: Report,
    screen: Report,
    native: Option<Native>,
}

struct Fit {
    /// Which segments show their own text.
    keep: Vec<bool>,
    /// One shared cap on every Screen narrative, in characters, when the
    /// reports alone would not fit.
    narrative_cap: Option<usize>,
}

/// The input budget a request has at the floor window, beside its
/// instruction: the same on every backend.
pub(crate) fn floor_budget(config: &Map<String, Value>) -> Option<u32> {
    let max_output_tokens = config
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())?;
    let preserved: u32 = ["user_instruction", "prompt", "_daily_location_text"]
        .iter()
        .filter_map(|key| config.get(*key).and_then(Value::as_str))
        .map(solstone_core_tokens::count)
        .sum();
    Some(
        solstone_core_local::generate::compute_input_budget(
            max_output_tokens,
            solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
        )
        .saturating_sub(preserved + FRAMING_TOKENS),
    )
}

/// Whether the request's own source would not fit the floor window.
pub(crate) fn over_floor(config: &Map<String, Value>) -> bool {
    floor_budget(config).is_some_and(|budget| {
        config
            .get("transcript")
            .and_then(Value::as_str)
            .is_some_and(|text| solstone_core_tokens::count(text) > budget)
    })
}

fn render_head(part: &Part, cap: Option<usize>) -> String {
    let screen = part.screen.model_value_capped(cap);
    let reports = json!({"sense": part.sense.model_value(), "screen": screen});
    format!(
        "{}\n\n### Processed segment reports\n{reports}\n\n",
        part.heading
    )
}

fn assemble(
    parts: &[Part],
    keep: &[bool],
    cap: Option<usize>,
) -> (String, Vec<Option<solstone_core_transcripts::EntrySource>>) {
    let mut text = REPORT_NOTE.to_owned();
    let mut sources = Vec::new();
    for (part, kept) in parts.iter().zip(keep) {
        text.push_str(&render_head(part, cap));
        match &part.native {
            Some((native_text, native_sources)) if *kept => {
                let offset = text.len();
                text.push_str(native_text);
                sources.extend(native_sources.iter().cloned().map(|source| {
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
            Some(_) => text.push_str(DROPPED_NOTE),
            None => {}
        }
        text.push_str("\n\n");
    }
    (text, sources)
}

/// Fit the packet to the floor window's input budget. Work shows every
/// segment's own text or none (a single terminal segment can exceed the
/// floor). Conversation keeps whole segments' own text, as many as fit, spread
/// evenly across the activity, so who said what and in what order survives
/// inside each, and every other segment is told by its report. When the
/// reports alone do not fit, every Screen narrative shares one cap, cut at a
/// sentence; Sense summaries and topics stay whole. Past that, the generic
/// fitter is the last-resort guard.
fn fit_to_floor(config: &Map<String, Value>, parts: &[Part]) -> Fit {
    let all = vec![true; parts.len()];
    let whole = Fit {
        keep: all.clone(),
        narrative_cap: None,
    };
    let Some(budget) = floor_budget(config) else {
        return whole;
    };
    let fits = |keep: &[bool], cap: Option<usize>| {
        solstone_core_tokens::count(&assemble(parts, keep, cap).0) <= budget
    };
    if fits(&all, None) {
        return whole;
    }
    let by_segment = config.get("name").and_then(Value::as_str) == Some("conversation");
    let keep = if by_segment {
        let candidates: Vec<usize> = (0..parts.len())
            .filter(|i| parts[*i].native.is_some())
            .collect();
        let spread = |k: usize| {
            let mut keep = vec![false; parts.len()];
            for i in 0..k {
                keep[candidates[(2 * i + 1) * candidates.len() / (2 * k)]] = true;
            }
            keep
        };
        if fits(&spread(0), None) {
            let (mut low, mut high) = (0, candidates.len());
            while low < high {
                let mid = low + (high - low).div_ceil(2);
                if fits(&spread(mid), None) {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            return Fit {
                keep: spread(low),
                narrative_cap: None,
            };
        }
        vec![false; parts.len()]
    } else {
        all
    };
    let longest = parts
        .iter()
        .map(|part| part.screen.narrative_chars())
        .max()
        .unwrap_or(0);
    let (mut low, mut high) = (0, longest);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(&keep, Some(mid)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Fit {
        keep,
        narrative_cap: Some(low),
    }
}

/// `text` cut to at most `cap` characters at the end of a sentence, or at a
/// word with an ellipsis when no sentence ends in reach.
fn cut_at_sentence(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_owned();
    }
    let head: String = text.chars().take(cap).collect();
    if let Some(end) = head
        .char_indices()
        .filter(|(i, c)| {
            matches!(c, '.' | '!' | '?')
                && head[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(i, c)| i + c.len_utf8())
        .next_back()
    {
        return head[..end].to_owned();
    }
    match head.rfind(char::is_whitespace) {
        Some(end) if end > 0 => format!("{}…", head[..end].trim_end()),
        _ => String::new(),
    }
}

/// Who spoke in the activity, counted from the speech lines exactly as they are
/// rendered for the model, or `None` when it holds no speech. Named voices are
/// listed most first, at most `MAX_NAMED_VOICES`.
pub(crate) fn speaker_count(
    transcript: &str,
    sources: &[Option<solstone_core_transcripts::EntrySource>],
) -> Option<String> {
    let (mut own, mut numbered) = (0_usize, 0_usize);
    let mut named: Vec<(String, usize)> = Vec::new();
    for source in sources
        .iter()
        .flatten()
        .filter(|source| source.is_speech_transcript())
    {
        let Some(text) = transcript.get(source.rendered_range.clone()) else {
            continue;
        };
        for label in text.lines().filter_map(speech_label) {
            if label == solstone_core_transcripts::OWNER_VOICE {
                own += 1;
            } else if label
                .strip_prefix("Speaker ")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            {
                numbered += 1;
            } else if let Some(entry) = named.iter_mut().find(|(name, _)| name == label) {
                entry.1 += 1;
            } else {
                named.push((label.to_owned(), 1));
            }
        }
    }
    let total = own + numbered + named.iter().map(|(_, n)| n).sum::<usize>();
    if total == 0 {
        return None;
    }
    named.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut rows = vec![
        "Who spoke in this activity, counted by the journal from its speech lines:".to_owned(),
    ];
    rows.push(if own > 0 {
        format!("- You: {own} of {total} lines.")
    } else {
        format!("- You: none of the {total} lines. Nothing said in this activity is yours.")
    });
    if !named.is_empty() {
        let list = named
            .iter()
            .take(MAX_NAMED_VOICES)
            .map(|(name, n)| format!("{name} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(format!("- Named voices: {list}."));
    }
    if numbered > 0 {
        rows.push(format!(
            "- Unnamed voices (\"Speaker\" and a number): {numbered} lines. The numbers restart in each segment, so the same number in two segments can be two different people."
        ));
    }
    rows.push("Every line that is not under `You:` was said by someone else.".to_owned());
    Some(rows.join("\n"))
}

const MAX_NAMED_VOICES: usize = 12;

/// The speaker label of one rendered speech line (`[12:00:01] Mina: …`, with an
/// optional parenthesised note before the label), or `None`.
fn speech_label(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let rest = &rest[rest.find("] ")? + 2..];
    let rest = match rest.strip_prefix('(') {
        Some(noted) => noted[noted.find(") ")? + 2..].trim_start(),
        None => rest,
    };
    let label = &rest[..rest.find(": ")?];
    (!label.is_empty() && label.chars().count() <= 60 && !label.contains(['[', ']']))
        .then_some(label)
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
    fn only_the_story_talents_get_an_alternate_and_large_raw_provenance_is_not_required() {
        let root = TempDir::new().unwrap();
        let path = segment(root.path(), "desktop", "090000_300", &"RAW".repeat(25_000));
        write(&path, "screen.json", json!({"narrative":"Compact report"}));
        let mut config = config(&["090000_300"]);
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(candidate.compact_transcript.contains("Compact report"));
        assert!(candidate.compact_transcript.contains("Speech RAW"));
        for name in ["screen", "sense", "participation"] {
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

    fn long_segment(root: &Path, key: &str, marker: &str, words: usize, narrative: &str) {
        let path = segment(root, "desktop", key, marker);
        let speech = (0..words)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            path.join("audio.jsonl"),
            json!({"start": "00:00:01", "text": format!("{marker} {speech}")}).to_string(),
        )
        .unwrap();
        write(
            &path,
            "sense.json",
            json!({"activity_summary": format!("SUMMARY_{marker}"), "topics":["t"]}),
        );
        write(&path, "screen.json", json!({"narrative": narrative}));
    }

    fn key(i: usize) -> String {
        format!("{:02}{:02}00_300", 9 + i / 12, (i % 12) * 5)
    }

    fn floor_config(name: &str, keys: &[&str]) -> Map<String, Value> {
        let mut config = config(keys);
        config.insert("name".into(), json!(name));
        config.insert("max_output_tokens".into(), json!(1536));
        config.insert("user_instruction".into(), json!("Tell the story."));
        config
    }

    #[test]
    fn conversation_over_the_floor_keeps_whole_segments_spread_evenly() {
        let root = TempDir::new().unwrap();
        let keys: Vec<String> = (0..8).map(key).collect();
        for (i, key) in keys.iter().enumerate() {
            long_segment(root.path(), key, &format!("SEG{i}"), 2_500, "Screen view.");
        }
        let keys = keys.iter().map(String::as_str).collect::<Vec<_>>();
        let config = floor_config("conversation", &keys);
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        let budget = floor_budget(&config).unwrap();
        assert!(solstone_core_tokens::count(&candidate.compact_transcript) <= budget);
        let shown: Vec<bool> = candidate.metadata["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| segment["native_shown"].as_bool().unwrap())
            .collect();
        let kept: Vec<usize> = (0..8).filter(|i| shown[*i]).collect();
        assert!(!kept.is_empty() && kept.len() < 8, "{shown:?}");
        // Every segment keeps its report; kept ones also keep their own words,
        // and the kept ones are spread across the activity, not bunched.
        for (i, shown) in shown.iter().enumerate() {
            assert!(
                candidate
                    .compact_transcript
                    .contains(&format!("SUMMARY_SEG{i}"))
            );
            assert_eq!(
                candidate.compact_transcript.contains(&format!("SEG{i} w0")),
                *shown
            );
        }
        assert!(
            kept.iter().any(|i| *i < 4) && kept.iter().any(|i| *i >= 4),
            "{kept:?}"
        );
        assert_eq!(
            candidate.compact_transcript.matches(DROPPED_NOTE).count(),
            8 - kept.len()
        );
        assert_eq!(candidate.metadata["native_transcripts_preserved"], false);
    }

    #[test]
    fn a_story_packet_that_fits_the_floor_is_left_whole() {
        let root = TempDir::new().unwrap();
        long_segment(root.path(), "090000_300", "ONLY", 50, "Screen view.");
        let config = floor_config("conversation", &["090000_300"]);
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(!candidate.compact_transcript.contains(DROPPED_NOTE));
        assert_eq!(candidate.metadata["native_transcripts_preserved"], true);
        assert_eq!(
            candidate.metadata["screen_narrative_cap_chars"],
            Value::Null
        );
    }

    #[test]
    fn work_over_the_floor_keeps_every_segment_and_shares_one_narrative_cap() {
        let root = TempDir::new().unwrap();
        let sentence = "The page lists the quarterly figures in detail. ";
        let keys: Vec<String> = (0..12).map(key).collect();
        for (i, key) in keys.iter().enumerate() {
            long_segment(
                root.path(),
                key,
                &format!("W{i}"),
                20,
                &sentence.repeat(800),
            );
        }
        let keys = keys.iter().map(String::as_str).collect::<Vec<_>>();
        let config = floor_config("work", &keys);
        let candidate = prepare(root.path(), &config).unwrap().unwrap();
        assert!(
            solstone_core_tokens::count(&candidate.compact_transcript)
                <= floor_budget(&config).unwrap()
        );
        let cap = candidate.metadata["screen_narrative_cap_chars"]
            .as_u64()
            .unwrap();
        assert!(cap > 0 && (cap as usize) < sentence.len() * 800);
        for i in 0..12 {
            assert!(
                candidate
                    .compact_transcript
                    .contains(&format!("SUMMARY_W{i}"))
            );
            assert!(candidate.compact_transcript.contains(&format!("W{i} w0")));
        }
        assert!(!candidate.compact_transcript.contains(DROPPED_NOTE));
        assert!(
            candidate
                .compact_transcript
                .contains("narrative_cut_to_fit")
        );
    }

    #[test]
    fn a_narrative_is_cut_at_a_sentence_or_a_word() {
        assert_eq!(cut_at_sentence("One. Two. Three.", 11), "One. Two.");
        assert_eq!(cut_at_sentence("Short.", 40), "Short.");
        assert_eq!(
            cut_at_sentence("no sentence end here at all", 14),
            "no sentence…"
        );
        assert_eq!(cut_at_sentence("v1.2 is out. Then more.", 8), "v1.2 is…");
        assert_eq!(cut_at_sentence("word", 2), "");
    }

    #[test]
    fn speaker_count_reads_the_labels_the_model_reads() {
        let transcript = "## seg\n[09:00:01] You: hi\n[09:00:02] Mina Okafor: hello\n[09:00:03] (laughing) Speaker 2: ha\n[09:00:04] Mina Okafor: so\n[09:00:05] Speaker 1: ok\nnot a line\n";
        let source = solstone_core_transcripts::EntrySource {
            path: "audio.jsonl".into(),
            sha256: String::new(),
            rendered_range: 0..transcript.len(),
            stream: Some("phone".into()),
            file: "audio.jsonl".into(),
            records: Vec::new(),
            unparsed_lines: 0,
            chunks: Vec::new(),
            chunk_offsets: Vec::new(),
            voice_named: Default::default(),
        };
        let count = speaker_count(transcript, &[Some(source.clone())]).unwrap();
        assert!(count.contains("- You: 1 of 5 lines."), "{count}");
        assert!(count.contains("- Named voices: Mina Okafor 2."), "{count}");
        assert!(
            count.contains("Speaker\" and a number): 2 lines."),
            "{count}"
        );
        let silent = "## seg\n[09:00:02] Mina Okafor: hello\n";
        let mut source = source;
        source.rendered_range = 0..silent.len();
        let count = speaker_count(silent, &[Some(source.clone())]).unwrap();
        assert!(
            count.contains("- You: none of the 1 lines. Nothing said in this activity is yours.")
        );
        source.file = "screen.jsonl".into();
        assert!(speaker_count(silent, &[Some(source)]).is_none());
        assert!(speaker_count(silent, &[None]).is_none());
    }
}
