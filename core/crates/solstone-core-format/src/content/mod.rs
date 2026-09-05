// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod action_logs;
mod activities;
mod ai_chat;
mod audio;
mod browser;
mod day_accumulator;
mod documents;
mod events;
mod facet_entities;
mod imports;
mod morning_briefing;
mod observations;
mod projections;
mod raw_screen;
mod screen;
mod sense;
mod shape;
mod talent_projections;

use serde_json::{Map, Value};

use crate::chunker::format_markdown;
use crate::matcher::{PatternSpec, Resolver, patterns_for_root as filter_patterns_for_root};

pub use crate::matcher::PatternRoot;
pub use projections::{render_browser_text, render_morning_briefing_text, render_raw_screen_text};
pub use shape::{SHAPE_SIDECAR_BASENAME, parse_shape_name, resolve_content_shape};
pub use talent_projections::{
    TalentTextProjection, iter_talent_text_projections, talent_projection_map,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Markdown,
    Event,
    Activity,
    ActionLog,
    StructuredImport,
    AiChat,
    Browser,
    DayAccumulator,
    FacetEntity,
    Observation,
    Documents,
    Screen,
    Sense,
    MorningBriefing,
}

/// Content that can be rendered for direct consumption but is deliberately
/// excluded from the search index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPerceptFamily {
    Audio,
    RawScreen,
}

#[cfg(test)]
const ALL_FAMILIES: [Family; 14] = [
    Family::Markdown,
    Family::Event,
    Family::Activity,
    Family::ActionLog,
    Family::StructuredImport,
    Family::AiChat,
    Family::Browser,
    Family::DayAccumulator,
    Family::FacetEntity,
    Family::Observation,
    Family::Documents,
    Family::Screen,
    Family::Sense,
    Family::MorningBriefing,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentResolution {
    Indexed(Family),
    Unindexed(RawPerceptFamily),
    IndexedElsewhere,
    Unrecognized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnindexedReason {
    RawPercept(RawPerceptFamily),
    IndexedElsewhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccurrenceTimeMs(pub i64);

impl From<i64> for OccurrenceTimeMs {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChunk {
    pub content: String,
    pub occurrence_time_ms: Option<OccurrenceTimeMs>,
    pub source: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedChunks {
    pub chunks: Vec<IndexChunk>,
    pub agent_override: Option<String>,
    pub header: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

/// Screen-talent rendering plus projector-owned tmux chunk identity.
///
/// `tmux_chunk_indices` is a side channel: owner content can contain any text,
/// so rendered Markdown must never be parsed to rediscover these boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenTalentRawScreen {
    pub chunks: Vec<IndexChunk>,
    pub agent_override: Option<String>,
    pub header: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub tmux_chunk_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyPattern {
    pub pattern: &'static str,
    pub family: Family,
    pub root: PatternRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnownUnindexedPattern {
    pub pattern: &'static str,
    pub root: PatternRoot,
    pub reason: UnindexedReason,
}

pub(crate) const INDEX_FAMILY_PATTERNS: &[FamilyPattern] = &[
    FamilyPattern {
        pattern: "*/talents/*.md",
        family: Family::Markdown,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/talents/sense.json",
        family: Family::Sense,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/talents/documents.json",
        family: Family::Documents,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/talents/screen.json",
        family: Family::Screen,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/talents/morning_briefing.json",
        family: Family::MorningBriefing,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/talents/*.jsonl",
        family: Family::DayAccumulator,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/talents/*.md",
        family: Family::Markdown,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/talents/*/*.md",
        family: Family::Markdown,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.*/*/*_transcript.md",
        family: Family::Markdown,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.*/*/imported.md",
        family: Family::Markdown,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/*/*/browser_*.jsonl",
        family: Family::Browser,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.*/imported.jsonl",
        family: Family::StructuredImport,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.chatgpt/*/conversation_transcript.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.claude/*/conversation_transcript.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.gemini/*/conversation_transcript.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.text/*/conversation_transcript.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.chatgpt/*/imported_audio.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.claude/*/imported_audio.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "*/import.gemini/*/imported_audio.jsonl",
        family: Family::AiChat,
        root: PatternRoot::DayRooted,
    },
    FamilyPattern {
        pattern: "config/actions/*.jsonl",
        family: Family::ActionLog,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/events/*.jsonl",
        family: Family::Event,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/entities/*/observations.jsonl",
        family: Family::Observation,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/entities/*.jsonl",
        family: Family::FacetEntity,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/activities/*.jsonl",
        family: Family::Activity,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/logs/*.jsonl",
        family: Family::ActionLog,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/activities/*/*/*.md",
        family: Family::Markdown,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "facets/*/news/*.md",
        family: Family::Markdown,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "reflections/weekly/*.md",
        family: Family::Markdown,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "imports/*/summary.md",
        family: Family::Markdown,
        root: PatternRoot::Structural,
    },
    FamilyPattern {
        pattern: "apps/*/talents/*.md",
        family: Family::Markdown,
        root: PatternRoot::Structural,
    },
];

pub(crate) const KNOWN_UNINDEXED_PATTERNS: &[KnownUnindexedPattern] = &[
    KnownUnindexedPattern {
        pattern: "entities/*/entity.json",
        root: PatternRoot::Structural,
        reason: UnindexedReason::IndexedElsewhere,
    },
    KnownUnindexedPattern {
        pattern: "*/*/*/audio.jsonl",
        root: PatternRoot::DayRooted,
        reason: UnindexedReason::RawPercept(RawPerceptFamily::Audio),
    },
    KnownUnindexedPattern {
        pattern: "*/*/*/*_audio.jsonl",
        root: PatternRoot::DayRooted,
        reason: UnindexedReason::RawPercept(RawPerceptFamily::Audio),
    },
    KnownUnindexedPattern {
        pattern: "*/*/*/*_transcript.jsonl",
        root: PatternRoot::DayRooted,
        reason: UnindexedReason::RawPercept(RawPerceptFamily::Audio),
    },
    KnownUnindexedPattern {
        pattern: "*/*/*/screen.jsonl",
        root: PatternRoot::DayRooted,
        reason: UnindexedReason::RawPercept(RawPerceptFamily::RawScreen),
    },
    KnownUnindexedPattern {
        pattern: "*/*/*/*_screen.jsonl",
        root: PatternRoot::DayRooted,
        reason: UnindexedReason::RawPercept(RawPerceptFamily::RawScreen),
    },
];

impl PatternSpec<Family> for FamilyPattern {
    fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn root(&self) -> PatternRoot {
        self.root
    }

    fn value(&self) -> Family {
        self.family
    }
}

impl PatternSpec<UnindexedReason> for KnownUnindexedPattern {
    fn pattern(&self) -> &'static str {
        self.pattern
    }

    fn root(&self) -> PatternRoot {
        self.root
    }

    fn value(&self) -> UnindexedReason {
        self.reason
    }
}

static CONTENT_RESOLVER: Resolver<Family, FamilyPattern> = Resolver::new(INDEX_FAMILY_PATTERNS);
static UNINDEXED_RESOLVER: Resolver<UnindexedReason, KnownUnindexedPattern> =
    Resolver::new(KNOWN_UNINDEXED_PATTERNS);

pub fn classify(rel: &str) -> ContentResolution {
    if let Some(family) = CONTENT_RESOLVER.resolve(rel) {
        return ContentResolution::Indexed(family);
    }
    match UNINDEXED_RESOLVER.resolve(rel) {
        Some(UnindexedReason::RawPercept(family)) => ContentResolution::Unindexed(family),
        Some(UnindexedReason::IndexedElsewhere) => ContentResolution::IndexedElsewhere,
        None => ContentResolution::Unrecognized,
    }
}

pub fn patterns_for_root(root: PatternRoot) -> impl Iterator<Item = &'static FamilyPattern> {
    filter_patterns_for_root(INDEX_FAMILY_PATTERNS, root)
}

/// Render an indexed non-Markdown content family from already-parsed records.
///
/// `Family::Markdown` has no record-shaped representation. Use
/// [`produce_chunks`] for Markdown text instead.
pub fn produce_chunks_by_shape(
    family: Family,
    rel: Option<&str>,
    records: &[JsonObject],
) -> ProducedChunks {
    let rel_text = rel.unwrap_or("");
    match family {
        // Markdown renders from text, not records, so there is nothing to
        // produce here. `produce_chunks` guards it before delegating — but this
        // is a public entry point, and an outside caller that resolved a
        // markdown shape reaches it directly. That caller is precisely the one
        // this function exists to serve, so report the misuse rather than
        // aborting their process: an empty render carrying an error is
        // recoverable, a panic in a library is not.
        Family::Markdown => ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: Some(
                "markdown renders from text, not records; call produce_chunks with the file text"
                    .to_string(),
            ),
            warnings: Vec::new(),
        },
        Family::Event => events::render(rel_text, records),
        Family::Activity => activities::render(rel, records),
        Family::ActionLog => action_logs::render(rel_text, records),
        Family::StructuredImport => imports::render(records),
        Family::AiChat => ai_chat::render(rel_text, records),
        Family::Browser => browser::render(records),
        Family::DayAccumulator => day_accumulator::render(rel_text, records),
        Family::FacetEntity => facet_entities::render(rel_text, records),
        Family::Observation => observations::render(rel_text, records),
        Family::Documents => documents::render(records),
        Family::Screen => screen::render(records),
        Family::Sense => sense::render(records),
        Family::MorningBriefing => morning_briefing::render(records),
    }
}

pub fn produce_chunks(family: Family, rel: &str, text: &str) -> ProducedChunks {
    if family == Family::Markdown {
        let formatted = format_markdown(text);
        return ProducedChunks {
            chunks: formatted
                .chunks
                .into_iter()
                .map(|chunk| IndexChunk {
                    content: chunk.markdown,
                    occurrence_time_ms: None,
                    source: None,
                })
                .collect(),
            agent_override: None,
            header: None,
            error: None,
            warnings: formatted.warnings,
        };
    }

    let records = parse_records_for_family(family, text);
    produce_chunks_by_shape(family, Some(rel), &records)
}

/// Render a raw percept from already-parsed records outside the indexed
/// content-family pipeline.
pub fn produce_raw_percept_chunks_by_shape(
    family: RawPerceptFamily,
    rel: Option<&str>,
    records: &[JsonObject],
) -> ProducedChunks {
    let rel = rel.unwrap_or("");
    match family {
        RawPerceptFamily::Audio => audio::render(rel, records),
        RawPerceptFamily::RawScreen => raw_screen::render(rel, records),
    }
}

/// Render a raw percept outside the indexed content-family pipeline.
pub fn produce_raw_percept_chunks(
    family: RawPerceptFamily,
    rel: &str,
    text: &str,
) -> ProducedChunks {
    let records = parse_jsonl_objects(text);
    produce_raw_percept_chunks_by_shape(family, Some(rel), &records)
}

/// Render raw screen records for the Screen talent's private input projection.
///
/// The ordinary raw-screen renderer remains the owner/index/display contract.
/// This separate entry point may compact a producer-typed tmux envelope because
/// its output is used only as model input for the Screen talent.
pub fn produce_screen_talent_raw_screen_chunks(rel: &str, text: &str) -> ScreenTalentRawScreen {
    let records = parse_jsonl_objects(text);
    raw_screen::render_for_screen_talent(rel, &records)
}

pub type JsonObject = Map<String, Value>;

pub(super) fn recorded_chunk(
    content: String,
    occurrence_time_ms: i64,
    source: &JsonObject,
) -> IndexChunk {
    IndexChunk {
        content,
        occurrence_time_ms: Some(OccurrenceTimeMs(occurrence_time_ms)),
        source: Some(source.clone()),
    }
}

fn parse_jsonl_objects(text: &str) -> Vec<JsonObject> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Object(record)) => Some(record),
                Ok(_) | Err(_) => None,
            }
        })
        .collect()
}

// Whole-file JSON talent outputs are intentionally infallible. Python writes
// files(path, mtime) after _index_file returns, even on JSONDecodeError; native
// scan.rs writes that row only from index_file's Ok arm. Malformed, empty, or
// non-object content must therefore render zero chunks instead of becoming an
// indexing error, or the files table diverges.
fn parse_json_object(text: &str) -> Vec<JsonObject> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(record)) => vec![record],
        Ok(_) | Err(_) => Vec::new(),
    }
}

fn parse_records_for_family(family: Family, text: &str) -> Vec<JsonObject> {
    match family {
        Family::Documents | Family::Screen | Family::Sense | Family::MorningBriefing => {
            parse_json_object(text)
        }
        _ => parse_jsonl_objects(text),
    }
}

fn json_falsy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => !value,
        Some(Value::Number(value)) => value.as_f64() == Some(0.0),
        Some(Value::String(value)) => value.is_empty(),
        Some(Value::Array(value)) => value.is_empty(),
        Some(Value::Object(value)) => value.is_empty(),
    }
}

fn clean_value(value: Option<&Value>) -> String {
    if json_falsy(value) {
        String::new()
    } else {
        value
            .map(display_value)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

fn json_truthy(value: Option<&Value>) -> bool {
    !json_falsy(value)
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn truthy_display(record: &JsonObject, key: &str) -> Option<String> {
    let value = record.get(key)?;
    if json_falsy(Some(value)) {
        None
    } else {
        Some(display_value(value))
    }
}

fn stripped_truthy_display(record: &JsonObject, key: &str) -> Option<String> {
    let value = record.get(key)?;
    if json_falsy(Some(value)) {
        return None;
    }
    let stripped = display_value(value).trim().to_string();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

fn display_or_default(record: &JsonObject, key: &str, default: &str) -> String {
    truthy_display(record, key).unwrap_or_else(|| default.to_string())
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut result = String::new();
    result.extend(first.to_uppercase());
    result.push_str(&chars.as_str().to_lowercase());
    result
}

fn titleize(value: &str) -> String {
    value
        .replace('_', " ")
        .split_whitespace()
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_string(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use glob::{MatchOptions, Pattern};

    use crate::chunker::test_support::{
        OVERSIZED_SIZE_NORMALIZATION, markdown_fixture, normalize_tokens, rust_tokenize, strings,
        token_comparison_enabled,
    };

    use super::*;

    fn produce_chunks(family: Family, rel: &str, text: &str) -> ProducedChunks {
        super::produce_chunks(family, rel, text)
    }

    /// Why a case is allowed to differ from the reference corpus.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Divergence {
        /// This crate is right and the reference was not. The recorded native
        /// value is the contract from here on.
        Accepted,
        /// This crate is wrong. The entry pins today's behaviour so it cannot
        /// drift further while the fix is outstanding, and keeps the difference
        /// visible instead of letting a green gate imply agreement.
        Defect,
    }

    #[derive(Debug)]
    struct DivergenceEntry {
        case: &'static str,
        kind: Divergence,
        native_chunks: &'static [&'static str],
        reason: &'static str,
    }

    /// Differences between this crate and the reference corpus, each one a
    /// decision rather than an accident. A mismatch that is not listed here
    /// fails the conformance test.
    const DIVERGENCES: &[DivergenceEntry] = &[
        DivergenceEntry {
            case: "event_nominal",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Meeting: Standup\n\n**Participants:** Alice, Bob\n\nDaily sync\n",
            ],
            reason: "reference emits a blank line for an absent header slot; whitespace only, \
                     tokenizes identically, and the native form is the cleaner record",
        },
        DivergenceEntry {
            case: "event_skip_is_title_only",
            kind: Divergence::Accepted,
            native_chunks: &["### Task: Kept\n\n"],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_nominal",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Person: Alice\n\nFriend from work\n\n**Tags:** tech, mentor\n**Also known as:** A, Al\n**Contact:** alice@example.com\n**Roles:** lead, reviewer\n**Empty Note:** \n",
            ],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_missing_fields",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Project: No Description\n\n*(No description available)*\n\n",
                "### Unknown: Unnamed\n\nOnly description\n\n",
            ],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_agent_from_slug_stem",
            kind: Divergence::Accepted,
            native_chunks: &["### Person: Slugged\n\n*(No description available)*\n\n"],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "activity_degenerate_rows_still_emit",
            kind: Divergence::Accepted,
            native_chunks: &["### Untitled activity", "### X\n- Activity: x"],
            reason: "the placeholder title for an activity with no title is sentence-cased here \
                     and lowercase in the reference; a heading an owner can see, so cased",
        },
        DivergenceEntry {
            case: "day_accumulator_nominal",
            kind: Divergence::Accepted,
            native_chunks: &["{\"ts\":1772000000000,\"summary\":\"steady morning\"}"],
            reason: "the accumulator chunk is the record re-serialized; the reference used \
                     json.dumps default separators and this crate emits compact JSON. \
                     Tokenizes identically",
        },
    ];

    fn divergence_for(case: &str) -> Option<&'static DivergenceEntry> {
        DIVERGENCES.iter().find(|entry| entry.case == case)
    }

    const RAW_PERCEPT_NON_RAISE_CASE_COUNT: usize = 15;
    const RAW_PERCEPT_RAISE_AS_ABSENT_CASE_COUNT: usize = 1;
    const RAW_PERCEPT_ERROR_CASES: &[&str] = &[
        "audio_row_missing_start_is_reported",
        "screen_frame_missing_timestamp_is_reported",
        "screen_first_row_without_timestamp_or_raw_is_skipped_not_metadata",
    ];

    fn rewrite_sol_urls(text: &str) -> String {
        const PREFIX: &str = "/app/sol/";
        let mut rewritten = String::new();
        let mut cursor = 0;
        while let Some(offset) = text[cursor..].find(PREFIX) {
            let start = cursor + offset;
            rewritten.push_str(&text[cursor..start]);
            let path_start = start + PREFIX.len();
            let path_end = text[path_start..]
                .find(is_url_delimiter)
                .map_or(text.len(), |offset| path_start + offset);
            let path = &text[path_start..path_end];
            rewritten
                .push_str(&rewrite_sol_path(path).unwrap_or_else(|| format!("{PREFIX}{path}")));
            cursor = path_end;
        }
        rewritten.push_str(&text[cursor..]);
        rewritten
    }

    fn is_url_delimiter(ch: char) -> bool {
        ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '<' | '>')
    }

    fn rewrite_sol_path(path: &str) -> Option<String> {
        let parts = path.split('/').collect::<Vec<_>>();
        if matches!(parts.as_slice(), [day, "talents", "facet_newsletter"] if day_key(day)) {
            return Some(format!("/app/thinking/#runs/{}/facet_newsletter", parts[0]));
        }
        if let Some((day, fragment)) = path.split_once('#')
            && day_key(day)
        {
            let parts = fragment.split('/').collect::<Vec<_>>();
            return match parts.as_slice() {
                [talent] if !talent.is_empty() => {
                    Some(format!("/app/thinking/#runs/{day}/{talent}"))
                }
                [talent, use_id] if !talent.is_empty() && !use_id.is_empty() => {
                    Some(format!("/app/thinking/#runs/{day}/{talent}/{use_id}"))
                }
                _ => None,
            };
        }
        if day_key(path) {
            return Some(format!("/app/thinking/#runs/{path}"));
        }
        (!path.is_empty() && !path.contains('/')).then(|| format!("/app/thinking/#runs/run/{path}"))
    }

    fn day_key(value: &str) -> bool {
        value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
    }

    fn compare_corpus_output(
        case: &serde_json::Value,
        id: &str,
        produced: ProducedChunks,
        use_divergences: bool,
        compare_error: bool,
        mismatches: &mut Vec<String>,
    ) {
        let expected = case["chunks"]
            .as_array()
            .expect("case chunks")
            .iter()
            .map(|chunk| rewrite_sol_urls(chunk["markdown"].as_str().expect("chunk markdown")))
            .collect::<Vec<_>>();
        let expected_refs = expected.iter().map(String::as_str).collect::<Vec<_>>();
        let actual: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        if actual != expected_refs {
            if use_divergences {
                match divergence_for(id) {
                    Some(entry) if actual == entry.native_chunks => {}
                    Some(entry) => mismatches.push(format!(
                        "{id}: recorded as a {:?} divergence, but the native output has since \
                         moved\n     recorded {:?}\n     actual   {actual:?}",
                        entry.kind, entry.native_chunks
                    )),
                    None => mismatches.push(format!(
                        "{id}: chunks differ and the difference is not recorded in DIVERGENCES\n\
                         \x20    reference {expected:?}\n     actual    {actual:?}"
                    )),
                }
            } else {
                mismatches.push(format!(
                    "{id}: chunks differ\n     reference {expected:?}\n     actual    {actual:?}"
                ));
            }
        }

        let expected_agent = case["agent"].as_str();
        if produced.agent_override.as_deref() != expected_agent {
            mismatches.push(format!(
                "{id}: agent differs — expected {:?}, actual {:?}",
                expected_agent, produced.agent_override
            ));
        }

        let expected_header = case.get("header").and_then(serde_json::Value::as_str);
        if produced.header.as_deref() != expected_header {
            mismatches.push(format!(
                "{id}: header differs — expected {:?}, actual {:?}",
                expected_header, produced.header
            ));
        }
        if compare_error {
            let expected_error = case.get("error").and_then(serde_json::Value::as_str);
            if produced.error.as_deref() != expected_error {
                mismatches.push(format!(
                    "{id}: error differs — expected {:?}, actual {:?}",
                    expected_error, produced.error
                ));
            }
        }

        for (index, (expected, actual)) in case["chunks"]
            .as_array()
            .expect("case chunks")
            .iter()
            .zip(&produced.chunks)
            .enumerate()
        {
            let expected_time = expected
                .get("timestamp_utc_ms")
                .and_then(serde_json::Value::as_i64)
                .map(OccurrenceTimeMs);
            if actual.occurrence_time_ms != expected_time {
                mismatches.push(format!(
                    "{id}:{index}: occurrence time differs — expected {expected_time:?}, actual {:?}",
                    actual.occurrence_time_ms
                ));
            }
            let expected_source = expected
                .get("source")
                .and_then(serde_json::Value::as_object);
            if actual.source.as_ref() != expected_source {
                mismatches.push(format!(
                    "{id}:{index}: source differs — expected {expected_source:?}, actual {:?}",
                    actual.source
                ));
            }
        }
    }

    /// Every family renders what the reference implementation recorded.
    ///
    /// The corpus is generated by `scripts/content_family_corpus.py` and is a
    /// frozen record: it cannot be regenerated once the reference tree stops
    /// being runnable, so a failure here is a question about this crate, never a
    /// prompt to re-derive the expectation.
    ///
    /// Cases with `raises` or a null `family` that do not carry `pending_family`
    /// are skipped by this assertion and covered by
    /// `content_family_corpus_records_known_divergences`. Cases carrying
    /// `pending_family` are rendered through `produce_raw_percept_chunks`: fifteen
    /// are compared in full, while the one reference-raising case is asserted
    /// against its documented as-if-content-absent behaviour.
    #[test]
    fn every_family_matches_the_reference_corpus() {
        let fixture_source = include_str!("../../../../fixtures/content_families.json");
        let legacy_url_count = fixture_source.matches("/app/sol/").count();
        assert_eq!(
            legacy_url_count, 1,
            "content families legacy Sol URL count: expected 1, actual {legacy_url_count}"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(fixture_source).expect("content family fixture parses");

        let cases = fixture["cases"].as_array().expect("fixture cases");
        assert!(!cases.is_empty(), "corpus is empty — nothing was compared");

        let mut compared = 0usize;
        let mut raw_percept_compared = 0usize;
        let mut raw_percept_raised = 0usize;
        let mut mismatches: Vec<String> = Vec::new();

        for case in cases {
            let id = case["id"].as_str().expect("case id");
            if let Some(name) = case
                .get("pending_family")
                .and_then(serde_json::Value::as_str)
            {
                let family = match parse_shape_name(name) {
                    Some(ContentResolution::Unindexed(family)) => family,
                    other => panic!("{id}: unknown raw percept family {name} ({other:?})"),
                };
                let rel = case["rel"].as_str().expect("case rel");
                let text = case["input_text"].as_str().expect("case input_text");
                let produced = produce_raw_percept_chunks(family, rel, text);
                if case.get("raises").is_some() {
                    assert_eq!(id, "screen_null_content_raises_in_the_reference");
                    assert_eq!(produced.agent_override.as_deref(), Some("screen"));
                    assert_eq!(produced.header.as_deref(), Some("# Frame Analyses"));
                    assert_eq!(produced.error, None);
                    assert_eq!(produced.chunks.len(), 1);
                    assert_eq!(produced.chunks[0].content, "### 09:00:03\n");
                    assert_eq!(
                        produced.chunks[0].occurrence_time_ms,
                        Some(OccurrenceTimeMs(1_772_614_803_000))
                    );
                    assert_eq!(
                        produced.chunks[0].source.as_ref(),
                        case["input"][1].as_object()
                    );
                    raw_percept_raised += 1;
                    continue;
                }
                let expects_error = RAW_PERCEPT_ERROR_CASES.contains(&id);
                assert_eq!(
                    case.get("error")
                        .and_then(serde_json::Value::as_str)
                        .is_some(),
                    expects_error,
                    "{id}: raw-percept corpus error classification"
                );
                assert_eq!(
                    produced.error.is_some(),
                    expects_error,
                    "{id}: raw-percept produced error classification"
                );
                compare_corpus_output(case, id, produced, false, true, &mut mismatches);
                raw_percept_compared += 1;
                continue;
            }
            if case.get("raises").is_some() {
                continue;
            }
            let Some(name) = case["family"].as_str() else {
                continue;
            };
            let family = match parse_shape_name(name) {
                Some(ContentResolution::Indexed(family)) => family,
                other => panic!("{id}: unknown family {name} ({other:?})"),
            };
            let rel = case["rel"].as_str().expect("case rel");
            let text = case["input_text"].as_str().expect("case input_text");
            let produced = super::produce_chunks(family, rel, text);
            compared += 1;
            compare_corpus_output(case, id, produced, true, false, &mut mismatches);
        }

        assert!(compared > 0, "no cases were compared");
        assert_eq!(
            raw_percept_compared, RAW_PERCEPT_NON_RAISE_CASE_COUNT,
            "raw-percept non-raise cases compared"
        );
        assert_eq!(
            raw_percept_raised, RAW_PERCEPT_RAISE_AS_ABSENT_CASE_COUNT,
            "raw-percept raises cases safely handled"
        );
        assert!(
            mismatches.is_empty(),
            "{} of {compared} compared cases diverge from the reference:\n  - {}",
            mismatches.len(),
            mismatches.join("\n  - ")
        );
    }

    /// Outstanding defects, stated rather than implied.
    ///
    /// The conformance test is green while a `Defect` divergence stands, because
    /// its job is to catch *drift*. This one exists so the green does not read as
    /// "this crate matches the reference" when part of it knowingly does not.
    /// Closing a defect means deleting its entry, not editing this count.
    #[test]
    fn outstanding_content_divergences_are_declared() {
        let defects: Vec<&DivergenceEntry> = DIVERGENCES
            .iter()
            .filter(|entry| entry.kind == Divergence::Defect)
            .collect();

        for entry in &defects {
            assert!(
                entry.reason.len() > 40,
                "{}: a defect divergence needs a reason that explains the owner-visible \
                 consequence, not a label",
                entry.case
            );
        }

        assert!(defects.is_empty(), "outstanding defects: {defects:?}");
    }

    /// The corpus carries exceptional cases whose notes are load-bearing, whether
    /// they remain skipped or are now rendered through `pending_family`. This
    /// asserts they stay present and stay described, so removing one is a decision
    /// rather than an accident.
    #[test]
    fn content_family_corpus_records_known_divergences() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/content_families.json"))
                .expect("content family fixture parses");
        let cases = fixture["cases"].as_array().expect("fixture cases");

        let divergences: Vec<&str> = cases
            .iter()
            .filter(|case| case.get("raises").is_some() || case["family"].is_null())
            .map(|case| case["id"].as_str().expect("case id"))
            .collect();

        assert!(
            !divergences.is_empty(),
            "the corpus records no divergences — either the generator regressed or \
             every difference was closed, and the second one needs saying out loud"
        );
        for case in cases
            .iter()
            .filter(|case| case.get("raises").is_some() || case["family"].is_null())
        {
            let id = case["id"].as_str().expect("case id");
            assert!(
                case.get("note")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "{id}: a recorded divergence must carry a note saying why"
            );
        }
    }

    #[test]
    fn classifies_indexable_families() {
        for (path, family) in [
            ("20240101/talents/flow.md", Family::Markdown),
            ("20260304/talents/pulse.jsonl", Family::DayAccumulator),
            (
                "20260304/default/090000_300/talents/sense.json",
                Family::Sense,
            ),
            (
                "20260304/default/090000_300/talents/documents.json",
                Family::Documents,
            ),
            (
                "20260304/default/090000_300/talents/screen.json",
                Family::Screen,
            ),
            (
                "20260304/talents/morning_briefing.json",
                Family::MorningBriefing,
            ),
            (
                "20260101/import.ics/imported.jsonl",
                Family::StructuredImport,
            ),
            (
                "20260101/import.claude/thread_a/conversation_transcript.jsonl",
                Family::AiChat,
            ),
            (
                "20260818/import.text/065323_5/conversation_transcript.jsonl",
                Family::AiChat,
            ),
            (
                "20260703/suze.browser/000141_317/browser_mail-google-com.jsonl",
                Family::Browser,
            ),
            ("facets/work/events/20240101.jsonl", Family::Event),
            ("facets/work/activities/20240101.jsonl", Family::Activity),
            ("facets/work/logs/20240101.jsonl", Family::ActionLog),
            ("facets/work/entities/foo.jsonl", Family::FacetEntity),
            (
                "facets/work/entities/alice/observations.jsonl",
                Family::Observation,
            ),
        ] {
            assert_eq!(classify(path), ContentResolution::Indexed(family), "{path}");
        }
    }

    #[test]
    fn classifies_non_indexed_and_unrecognized_paths() {
        assert_eq!(
            classify("20240101/default/123456_300/audio.jsonl"),
            ContentResolution::Unindexed(RawPerceptFamily::Audio)
        );
        assert_eq!(
            classify("entities/alice/entity.json"),
            ContentResolution::IndexedElsewhere
        );
        assert_eq!(classify("notes/foo.txt"), ContentResolution::Unrecognized);
    }

    #[test]
    fn structural_content_patterns_take_priority_over_day_rooted_patterns() {
        for (path, family) in [
            ("facets/work/logs/browser_mail.jsonl", Family::ActionLog),
            ("facets/work/events/browser_x.jsonl", Family::Event),
            ("facets/work/activities/browser_a.jsonl", Family::Activity),
            ("facets/chat/logs/chat.jsonl", Family::ActionLog),
            (
                "facets/import.claude/logs/conversation_transcript.jsonl",
                Family::ActionLog,
            ),
        ] {
            assert_eq!(classify(path), ContentResolution::Indexed(family), "{path}");
        }
    }

    /// An indexed family wins over a known-unindexed pattern that also matches.
    ///
    /// The three AI-chat legacy filenames match their own family pattern *and*
    /// `*/*/*/*_audio.jsonl`, and both are day-rooted, so namespace precedence
    /// cannot separate them. The first assertion in each iteration proves the
    /// collision is real rather than assumed — without it this test could pass
    /// against patterns that never overlap and prove nothing.
    ///
    /// Why it is worth pinning: an unindexed resolution is `continue`d by the
    /// scan with no warning and no skipped count, so if these three ever
    /// resolved unindexed, every imported ChatGPT, Claude and Gemini transcript
    /// would stop being searchable while the scan still reported success.
    ///
    /// ⚠ This pins the outcome, not a mechanism. Swapping the order in which
    /// `classify` consults the two tables does **not** break it — that was
    /// tried, and the full suite stayed green — so whatever decides this case
    /// is not table order alone. Do not read the test as guarding an ordering.
    ///
    /// The final assertion is the inverted twin: a path matching *only* the
    /// unindexed pattern must still resolve unindexed, so this cannot be
    /// satisfied by a resolver that never consults the unindexed table at all.
    #[test]
    fn an_indexed_family_wins_over_a_matching_unindexed_pattern() {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        for path in [
            "20260101/import.chatgpt/conv_b/imported_audio.jsonl",
            "20260101/import.claude/conv_b/imported_audio.jsonl",
            "20260101/import.gemini/conv_b/imported_audio.jsonl",
        ] {
            assert!(
                Pattern::new("*/*/*/*_audio.jsonl")
                    .expect("valid unindexed pattern")
                    .matches_path_with(Path::new(path), options),
                "{path} must genuinely collide, or this test proves nothing"
            );
            assert_eq!(
                classify(path),
                ContentResolution::Indexed(Family::AiChat),
                "{path}"
            );
        }

        let imported_text = "20260818/import.text/065323_5/conversation_transcript.jsonl";
        assert!(
            Pattern::new("*/*/*/*_transcript.jsonl")
                .expect("valid unindexed pattern")
                .matches_path_with(Path::new(imported_text), options),
            "{imported_text} must genuinely collide, or this test proves nothing"
        );
        assert_eq!(
            classify(imported_text),
            ContentResolution::Indexed(Family::AiChat),
            "{imported_text}"
        );

        assert_eq!(
            classify("20260101/default/123456_300/left_audio.jsonl"),
            ContentResolution::Unindexed(RawPerceptFamily::Audio),
        );
    }

    #[test]
    fn every_cross_namespace_intersection_has_the_expected_resolution() {
        let structural_json = [
            ("events", Family::Event),
            ("entities", Family::FacetEntity),
            ("activities", Family::Activity),
            ("logs", Family::ActionLog),
        ];
        let day_json = [
            "facets/chat/{structural}/chat.jsonl",
            "facets/x/{structural}/browser_x.jsonl",
            "facets/import.chatgpt/{structural}/conversation_transcript.jsonl",
            "facets/import.claude/{structural}/conversation_transcript.jsonl",
            "facets/import.gemini/{structural}/conversation_transcript.jsonl",
            "facets/import.chatgpt/{structural}/imported_audio.jsonl",
            "facets/import.claude/{structural}/imported_audio.jsonl",
            "facets/import.gemini/{structural}/imported_audio.jsonl",
        ];

        let mut json_intersections = 0;
        for (structural, family) in structural_json {
            for day_pattern in day_json {
                let path = day_pattern.replace("{structural}", structural);
                assert_eq!(
                    classify(&path),
                    ContentResolution::Indexed(family),
                    "structural JSON pattern must win for {path}"
                );
                json_intersections += 1;
            }
        }
        assert_eq!(json_intersections, 32);

        // Pre-existing overlaps. Both roots already classify these as Markdown,
        // so root-first matching must preserve their shared outcome.
        let markdown_intersections = [
            "facets/x/activities/talents/x/x.md",
            "facets/import.x/news/x_transcript.md",
            "facets/import.x/news/imported.md",
            "imports/talents/summary.md",
            "apps/import.x/talents/x_transcript.md",
            "apps/import.x/talents/imported.md",
        ];
        for path in markdown_intersections {
            assert_eq!(
                classify(path),
                ContentResolution::Indexed(Family::Markdown),
                "pre-existing Markdown overlap changed for {path}"
            );
        }
        assert_eq!(markdown_intersections.len(), 6);
    }

    #[test]
    fn registered_patterns_compile_and_cover_every_family() {
        for family in ALL_FAMILIES {
            assert!(
                INDEX_FAMILY_PATTERNS
                    .iter()
                    .any(|pattern| pattern.family == family),
                "{family:?} has no registered content pattern"
            );
        }
        for pattern in INDEX_FAMILY_PATTERNS {
            Pattern::new(pattern.pattern).expect("index family pattern compiles");
        }
        for pattern in KNOWN_UNINDEXED_PATTERNS {
            Pattern::new(pattern.pattern).expect("known-unindexed pattern compiles");
        }
    }

    #[test]
    fn markdown_producer_matches_python_oracle_tokens() {
        let fixture = markdown_fixture();
        for case in fixture["cases"].as_array().expect("fixture cases") {
            if !token_comparison_enabled(case) {
                continue;
            }
            let id = case["id"].as_str().expect("case id");
            let input = case["input"].as_str().expect("case input");
            let produced = produce_chunks(Family::Markdown, "20240101/talents/oracle.md", input);
            assert_eq!(
                produced.warnings,
                strings(&case["warnings"]),
                "{id} warnings"
            );
            assert_eq!(
                produced.chunks.len(),
                case["chunk_count"].as_u64().expect("chunk count") as usize,
                "{id} chunk count"
            );
            for (idx, expected_chunk) in case["chunks"]
                .as_array()
                .expect("case chunks")
                .iter()
                .enumerate()
            {
                let normalizations = strings(&expected_chunk["normalizations"]);
                let recorded_tokens = strings(&expected_chunk["tokens"]);
                assert_eq!(
                    normalize_tokens(
                        rust_tokenize(&produced.chunks[idx].content),
                        &normalizations
                    ),
                    recorded_tokens,
                    "{id}:{idx} producer tokens"
                );
            }
            assert_eq!(produced.agent_override, None, "{id} agent override");
        }
    }

    #[test]
    fn markdown_producer_ports_grouping_rules() {
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Tasks\n\nintro alpha\n\n- item one\n- item two\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            ["tasks", "intro", "alpha", "item", "one"]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            ["tasks", "intro", "alpha", "item", "two"]
        );
        assert!(
            !produced
                .chunks
                .iter()
                .any(|chunk| chunk.content.trim() == "# Tasks\n\nintro alpha")
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Tasks\n\n- item one\n- item two\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            ["tasks", "item", "one"]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            ["tasks", "item", "two"]
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Definitions\n\n- **alpha:** value one\n- ordinary note.\n- **beta:** value two\n- ordinary other.\n",
        );
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            [
                "definitions",
                "alpha",
                "value",
                "one",
                "ordinary",
                "note",
                "beta",
                "value",
                "two",
                "ordinary",
                "other"
            ]
        );
        assert_eq!(produced.agent_override, None);
        assert!(produced.warnings.is_empty());
    }

    #[test]
    fn markdown_producer_ports_table_and_heading_rules() {
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Root\n\n## Matrix\n\nintro alpha\n\n| Name | Value |\n| --- | --- |\n| beta | one |\n| gamma | two |\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            [
                "root", "matrix", "intro", "alpha", "name", "value", "beta", "one"
            ]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            [
                "root", "matrix", "intro", "alpha", "name", "value", "gamma", "two"
            ]
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Root\n\n## Empty\n\n| Name | Value |\n| --- | --- |\n",
        );
        assert!(produced.chunks.is_empty());
    }

    #[test]
    fn markdown_producer_drops_overlong_lines_and_stubs_oversized_chunks() {
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            &format!("# Long\n\n{}\n\nkept alpha\n", "z".repeat(2049)),
        );
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            produced.warnings,
            vec!["Dropped 1 line(s) exceeding 2048 chars during markdown sanitization"]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            ["long", "kept", "alpha"]
        );

        let oversized_line = "alpha ".repeat(300);
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            &format!(
                "# Big\n\n{}\n{}\n{}",
                oversized_line, oversized_line, oversized_line
            ),
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(
            produced.chunks[0]
                .content
                .contains("[Content too large to index:")
        );
        assert_eq!(
            normalize_tokens(
                rust_tokenize(&produced.chunks[0].content),
                &[OVERSIZED_SIZE_NORMALIZATION.to_string()]
            ),
            [
                "big",
                "content",
                "too",
                "large",
                "to",
                "index",
                "normalizedsize",
                "chars"
            ]
        );
    }

    #[test]
    fn jsonl_parser_skips_malformed_and_non_object_lines_for_all_jsonl_families() {
        let text = r#"
{"title":"Planning","type":"meeting"}
42
["not", "object"]
not json
{"title":"Review","type":"task"}
"#;
        let produced = produce_chunks(Family::Event, "facets/work/events/20240101.jsonl", text);
        assert_eq!(produced.chunks.len(), 2);
        assert!(produced.chunks[0].content.contains("Meeting: Planning"));
        assert!(produced.chunks[1].content.contains("Task: Review"));

        let produced = produce_chunks(
            Family::ActionLog,
            "config/actions/20240101.jsonl",
            r#"
42
not json
{"action":"identity_update","actor":"settings"}
"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(
            produced.chunks[0]
                .content
                .contains("Identity Update by settings")
        );

        let produced = produce_chunks(
            Family::Activity,
            "facets/work/activities/20240101.jsonl",
            r#"
42
not json
{"id":"coding_090000_300"}
"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("### Coding 090000 300"));
    }

    #[test]
    fn json_object_parser_is_infallible_for_talent_json_inputs() {
        assert_eq!(parse_json_object(r#"{"title":"Planning"}"#).len(), 1);
        for text in ["", "   ", "not json", "null", "42", r#""string""#, "[]"] {
            assert!(parse_json_object(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn text_wrappers_delegate_to_by_shape_renderers() {
        let event_text = r#"{"title":"Planning","type":"meeting"}"#;
        let event_records = parse_jsonl_objects(event_text);
        assert_eq!(
            super::produce_chunks(
                Family::Event,
                "facets/work/events/20260101.jsonl",
                event_text,
            ),
            produce_chunks_by_shape(
                Family::Event,
                Some("facets/work/events/20260101.jsonl"),
                &event_records,
            )
        );

        let screen_text = r#"{"timestamp":3,"content":{}}"#;
        let screen_records = parse_jsonl_objects(screen_text);
        assert_eq!(
            super::produce_raw_percept_chunks(
                RawPerceptFamily::RawScreen,
                "20260304/workstation/090000_300/screen.jsonl",
                screen_text,
            ),
            produce_raw_percept_chunks_by_shape(
                RawPerceptFamily::RawScreen,
                Some("20260304/workstation/090000_300/screen.jsonl"),
                &screen_records,
            )
        );
    }

    #[test]
    fn by_shape_activity_without_path_uses_the_pathless_header() {
        let records = parse_jsonl_objects(r#"{"title":"Planning"}"#);
        let produced = produce_chunks_by_shape(Family::Activity, None, &records);
        assert_eq!(produced.header.as_deref(), Some("# Activities"));
    }

    #[test]
    fn by_shape_raw_screen_uses_synthetic_records_with_a_real_path() {
        let records = parse_jsonl_objects(r#"{"timestamp":3,"content":{}}"#);
        let produced = produce_raw_percept_chunks_by_shape(
            RawPerceptFamily::RawScreen,
            Some("20260304/workstation/090000_300/screen.jsonl"),
            &records,
        );
        assert_eq!(produced.header.as_deref(), Some("# Frame Analyses"));
        assert_eq!(produced.chunks.len(), 1);
    }

    #[test]
    fn structured_import_agent_preserves_source_case_until_merge() {
        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            "",
        );
        assert_eq!(produced.agent_override, None);
        assert_eq!(produced.chunks.len(), 0);

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ics"));

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ICS"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ICS"));

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"entry_count":1}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.unknown"));
    }

    #[test]
    fn structured_import_skips_header_and_empty_generic_entries() {
        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"},"title":"Header"}
{"type":"generic"}"#,
        );
        assert_eq!(produced.chunks.len(), 0);

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"}}
{"type":"calendar_event","title":"Quarterly Planning","ts":"2026-01-01T09:30:00-07:00"}"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("Quarterly Planning"));
    }

    #[test]
    fn ai_chat_agent_comes_from_path_or_fallback() {
        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.claude"));

        let produced = produce_chunks(
            Family::AiChat,
            "20260101/misc/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ai_chat"));
    }

    #[test]
    fn ai_chat_indexes_only_non_empty_start_bearing_turns() {
        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3","imported":{"facet":"work"}}
{"start":"00:00:01","speaker":"User","text":"Hello"}
{"start":"00:00:02","speaker":"Assistant","text":""}
{"start":"00:00:03","speaker":"Assistant","text":"Hi there"}
{"speaker":"System","text":"metadata-like"}"#,
        );
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        assert_eq!(contents, vec!["**User:** Hello", "**Assistant:** Hi there"]);

        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}
{"start":"00:00:01","speaker":"User","text":""}"#,
        );
        assert_eq!(produced.chunks.len(), 0);
    }

    #[test]
    fn event_skip_predicate_is_title_only() {
        let produced = produce_chunks(
            Family::Event,
            "facets/work/events/20240101.jsonl",
            r#"{"type":"meeting"}
{"title":"","type":"meeting"}
{"title":"Standup","type":"meeting","participants":["Alice","Bob"],"summary":"Daily sync"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("event"));
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("### Meeting: Standup"));
        assert!(
            produced.chunks[0]
                .content
                .contains("**Participants:** Alice, Bob")
        );
        assert!(produced.chunks[0].content.contains("Daily sync"));
    }

    #[test]
    fn action_log_skip_predicate_is_action_only() {
        let produced = produce_chunks(
            Family::ActionLog,
            "config/actions/20240101.jsonl",
            r#"{"actor":"settings"}
{"action":"","actor":"settings"}
{"action":"identity_update","actor":"settings","source":"app","timestamp":"2025-12-16T07:33:05.135587+00:00","use_id":"123","params":{"name":"Alice"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("action"));
        assert_eq!(produced.chunks.len(), 1);
        assert!(
            produced.chunks[0]
                .content
                .contains("### Identity Update by settings")
        );
        assert!(
            produced.chunks[0]
                .content
                .contains("**Source:** app | **Time:** 07:33:05")
        );
        assert!(
            produced.chunks[0]
                .content
                .contains("**Talent:** [123](/app/thinking/#runs/run/123)")
        );
        assert!(produced.chunks[0].content.contains("- name: Alice"));
    }

    #[test]
    fn activity_objects_always_produce_chunks() {
        let produced = produce_chunks(
            Family::Activity,
            "facets/work/activities/20240101.jsonl",
            r#"{}
{"id":"x"}
{"title":"Launch sync","activity":"meeting","facet":"work","day":"20260418","segments":["090000_300"],"level_avg":0.5,"description":"Team sync","details":"Assigned owners","participation":[{"name":"Mina"}],"story":{"body":"Aligned on launch.","topics":["launch","owners"]},"hidden":true}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("activity"));
        assert_eq!(produced.chunks.len(), 3);
        assert!(produced.chunks[0].content.contains("### Untitled activity"));
        assert!(produced.chunks[1].content.contains("### X"));
        assert!(produced.chunks[2].content.contains("### Launch sync"));
        assert!(produced.chunks[2].content.contains("- Time: 09:00-09:05"));
        assert!(produced.chunks[2].content.contains("- Participation: Mina"));
        assert!(produced.chunks[2].content.contains("Aligned on launch."));
        assert!(
            produced.chunks[2]
                .content
                .contains("Topics: launch, owners")
        );
        assert!(produced.chunks[2].content.contains("- Hidden: yes"));
    }

    #[test]
    fn facet_entity_agent_comes_from_ascii_digit_file_stem() {
        for (rel, agent) in [
            ("facets/work/entities/20260304.jsonl", "entity:detected"),
            ("facets/work/entities/123.jsonl", "entity:detected"),
            ("facets/work/entities/99999999.jsonl", "entity:detected"),
            ("facets/work/entities/some-slug.jsonl", "entity:attached"),
        ] {
            let produced = produce_chunks(Family::FacetEntity, rel, "");
            assert_eq!(produced.agent_override.as_deref(), Some(agent), "{rel}");
            assert_eq!(produced.chunks.len(), 0, "{rel}");
        }
    }

    #[test]
    fn facet_entity_renderer_formats_entity_fields() {
        let produced = produce_chunks(
            Family::FacetEntity,
            "facets/work/entities/20260304.jsonl",
            r#"{"id":"alice","type":"Person","name":"Alice","description":"Friend from work","tags":["tech","mentor"],"aka":["A","Al"],"contact":"alice@example.com","roles":["lead","reviewer"],"empty_note":"","last_seen":"20260304","detached":true}
{"type":"Project","name":"No Description","description":""}
{"description":"Only description"}"#,
        );

        assert_eq!(produced.agent_override.as_deref(), Some("entity:detected"));
        assert_eq!(produced.chunks.len(), 3);
        let first = &produced.chunks[0].content;
        assert!(first.contains("### Person: Alice"));
        assert!(first.contains("Friend from work"));
        assert!(first.contains("**Tags:** tech, mentor"));
        assert!(first.contains("**Also known as:** A, Al"));
        assert!(first.contains("**Contact:** alice@example.com"));
        assert!(first.contains("**Roles:** lead, reviewer"));
        assert!(first.contains("**Empty Note:** "));
        assert!(!first.contains("Last Seen"));
        assert!(!first.contains("Detached"));
        assert!(
            produced.chunks[1]
                .content
                .contains("*(No description available)*")
        );
        assert!(produced.chunks[2].content.contains("### Unknown: Unnamed"));
    }

    #[test]
    fn observation_renderer_formats_source_day_when_truthy() {
        let produced = produce_chunks(
            Family::Observation,
            "facets/work/entities/alice/observations.jsonl",
            r#"{"content":"Prefers morning meetings","source_day":"20250113"}
{"content":"Expert in distributed systems"}
{"source_day":"20250114"}
{"content":"","source_day":""}"#,
        );

        assert_eq!(produced.agent_override.as_deref(), Some("observation"));
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        assert_eq!(
            contents,
            vec![
                "- Prefers morning meetings (observed: 20250113)",
                "- Expert in distributed systems",
                "-  (observed: 20250114)",
                "- ",
            ]
        );
    }
}
