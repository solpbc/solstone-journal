// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod action_logs;
mod activities;
mod ai_chat;
mod browser;
mod chat;
mod day_accumulator;
mod documents;
mod events;
mod facet_entities;
mod imports;
mod morning_briefing;
mod observations;
mod screen;
mod sense;

use std::path::Path;

use glob::{MatchOptions, Pattern};
use serde_json::{Map, Value};

use crate::chunker::format_markdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Markdown,
    Event,
    Activity,
    ActionLog,
    StructuredImport,
    AiChat,
    Chat,
    Browser,
    DayAccumulator,
    FacetEntity,
    Observation,
    Documents,
    Screen,
    Sense,
    MorningBriefing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChunk {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedChunks {
    pub chunks: Vec<IndexChunk>,
    pub agent_override: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternRoot {
    Structural,
    DayRooted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FamilyPattern {
    pub pattern: &'static str,
    pub family: Family,
    pub root: PatternRoot,
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
        pattern: "*/chat/*/chat.jsonl",
        family: Family::Chat,
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

pub fn classify(rel: &str) -> Option<Family> {
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let rel_path = Path::new(rel);
    for spec in INDEX_FAMILY_PATTERNS {
        let pattern = Pattern::new(spec.pattern).expect("index family pattern should be valid");
        if pattern.matches_path_with(rel_path, options) {
            return Some(spec.family);
        }
    }
    None
}

pub(crate) fn patterns_for_root(root: PatternRoot) -> impl Iterator<Item = &'static FamilyPattern> {
    INDEX_FAMILY_PATTERNS
        .iter()
        .filter(move |spec| spec.root == root)
}

pub fn produce_chunks(family: Family, rel: &str, text: &str) -> ProducedChunks {
    match family {
        Family::Markdown => {
            let formatted = format_markdown(text);
            ProducedChunks {
                chunks: formatted
                    .chunks
                    .into_iter()
                    .map(|chunk| IndexChunk {
                        content: chunk.markdown,
                    })
                    .collect(),
                agent_override: None,
                warnings: formatted.warnings,
            }
        }
        Family::Event => ProducedChunks {
            chunks: events::render(&parse_jsonl_objects(text)),
            agent_override: Some("event".to_string()),
            warnings: Vec::new(),
        },
        Family::Activity => ProducedChunks {
            chunks: activities::render(&parse_jsonl_objects(text)),
            agent_override: Some("activity".to_string()),
            warnings: Vec::new(),
        },
        Family::ActionLog => ProducedChunks {
            chunks: action_logs::render(&parse_jsonl_objects(text)),
            agent_override: Some("action".to_string()),
            warnings: Vec::new(),
        },
        Family::StructuredImport => imports::render(&parse_jsonl_objects(text)),
        Family::AiChat => ai_chat::render(rel, &parse_jsonl_objects(text)),
        Family::Chat => chat::render(&parse_jsonl_objects(text)),
        Family::Browser => browser::render(&parse_jsonl_objects(text)),
        Family::DayAccumulator => day_accumulator::render(rel, &parse_jsonl_objects(text)),
        Family::FacetEntity => facet_entities::render(rel, &parse_jsonl_objects(text)),
        Family::Observation => ProducedChunks {
            chunks: observations::render(&parse_jsonl_objects(text)),
            agent_override: Some("observation".to_string()),
            warnings: Vec::new(),
        },
        Family::Documents => documents::render(&parse_json_object(text)),
        Family::Screen => screen::render(&parse_json_object(text)),
        Family::Sense => sense::render(&parse_json_object(text)),
        Family::MorningBriefing => morning_briefing::render(&parse_json_object(text)),
    }
}

type JsonObject = Map<String, Value>;

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
    use crate::chunker::test_support::{
        OVERSIZED_SIZE_NORMALIZATION, markdown_fixture, normalize_tokens, rust_tokenize, strings,
        token_comparison_enabled,
    };

    use super::*;

    fn family_by_name(name: &str) -> Option<Family> {
        Some(match name {
            "Markdown" => Family::Markdown,
            "Event" => Family::Event,
            "Activity" => Family::Activity,
            "ActionLog" => Family::ActionLog,
            "StructuredImport" => Family::StructuredImport,
            "AiChat" => Family::AiChat,
            "Chat" => Family::Chat,
            "Browser" => Family::Browser,
            "DayAccumulator" => Family::DayAccumulator,
            "FacetEntity" => Family::FacetEntity,
            "Observation" => Family::Observation,
            "Documents" => Family::Documents,
            "Screen" => Family::Screen,
            "Sense" => Family::Sense,
            "MorningBriefing" => Family::MorningBriefing,
            _ => return None,
        })
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
        DivergenceEntry {
            case: "chat_turns_with_configured_labels",
            kind: Divergence::Defect,
            native_chunks: &[
                "**Owner** What did I do today?",
                "**Sol** You shipped the cable.",
            ],
            reason: "🔴 speaker labels are hardcoded here and resolved from journal config by \
                     the reference (identity.preferred / identity.name, agent.name). On a journal \
                     whose owner has set a name the two disagree on every owner turn, so a rescan \
                     through this crate rewrites the owner's name out of indexed chat. Labels must \
                     become an input; needs journal-config plumbing this crate does not have yet",
        },
        DivergenceEntry {
            case: "chat_turn_without_text_keeps_the_label",
            kind: Divergence::Defect,
            native_chunks: &["**Owner**"],
            reason: "same hardcoded-label defect as chat_turns_with_configured_labels",
        },
    ];

    fn divergence_for(case: &str) -> Option<&'static DivergenceEntry> {
        DIVERGENCES.iter().find(|entry| entry.case == case)
    }

    /// Every family renders what the reference implementation recorded.
    ///
    /// The corpus is generated by `scripts/content_family_corpus.py` and is a
    /// frozen record: it cannot be regenerated once the reference tree stops
    /// being runnable, so a failure here is a question about this crate, never a
    /// prompt to re-derive the expectation.
    ///
    /// Cases carrying `raises` record a reference behaviour deliberately not
    /// reproduced here, and cases with a null `family` record a dispatch fact for
    /// a shape this crate has no family for; both are skipped by this assertion
    /// and are covered by `content_family_corpus_records_known_divergences`.
    #[test]
    fn every_family_matches_the_reference_corpus() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/content_families.json"))
                .expect("content family fixture parses");

        let cases = fixture["cases"].as_array().expect("fixture cases");
        assert!(!cases.is_empty(), "corpus is empty — nothing was compared");

        let mut compared = 0usize;
        let mut mismatches: Vec<String> = Vec::new();

        for case in cases {
            let id = case["id"].as_str().expect("case id");
            if case.get("raises").is_some() {
                continue;
            }
            let Some(name) = case["family"].as_str() else {
                continue;
            };
            let family =
                family_by_name(name).unwrap_or_else(|| panic!("{id}: unknown family {name}"));
            let rel = case["rel"].as_str().expect("case rel");
            let text = case["input_text"].as_str().expect("case input_text");

            let produced = produce_chunks(family, rel, text);
            compared += 1;

            let expected: Vec<&str> = case["chunks"]
                .as_array()
                .expect("case chunks")
                .iter()
                .map(|chunk| chunk["markdown"].as_str().expect("chunk markdown"))
                .collect();
            let actual: Vec<&str> = produced
                .chunks
                .iter()
                .map(|chunk| chunk.content.as_str())
                .collect();
            if actual != expected {
                match divergence_for(id) {
                    // A recorded difference still has to hold exactly, so it can
                    // be revisited as a decision rather than drifting quietly.
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
            }

            let expected_agent = case["agent"].as_str();
            if produced.agent_override.as_deref() != expected_agent {
                mismatches.push(format!(
                    "{id}: agent differs — expected {:?}, actual {:?}",
                    expected_agent, produced.agent_override
                ));
            }
        }

        assert!(compared > 0, "no cases were compared");
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

        assert_eq!(
            defects.len(),
            2,
            "the declared defect count moved. Closing one is the goal — delete its entry. \
             Adding one is a decision that belongs in the build record, not a bumped number. \
             Currently declared: {:?}",
            defects.iter().map(|entry| entry.case).collect::<Vec<_>>()
        );
    }

    /// The corpus carries cases this crate deliberately does not reproduce.
    /// They are load-bearing: each one is a difference that would otherwise be
    /// rediscovered from scratch. This asserts they stay present and stay
    /// described, so removing one is a decision rather than an accident.
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
        assert_eq!(classify("20240101/talents/flow.md"), Some(Family::Markdown));
        assert_eq!(
            classify("20260304/talents/pulse.jsonl"),
            Some(Family::DayAccumulator)
        );
        assert_eq!(
            classify("20260304/talents/pulse.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/sense.json"),
            Some(Family::Sense)
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/documents.json"),
            Some(Family::Documents)
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/screen.json"),
            Some(Family::Screen)
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/sense.jsonl"),
            None
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/documents.jsonl"),
            None
        );
        assert_eq!(
            classify("20260304/default/090000_300/talents/screen.jsonl"),
            None
        );
        assert_eq!(
            classify("20260304/talents/morning_briefing.json"),
            Some(Family::MorningBriefing)
        );
        assert_eq!(
            classify("20240101/default/123456_300/talents/audio.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("20240101/default/123456_300/talents/work/audio.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("20260101/import.ics/090000_300/event_transcript.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("20260101/import.ics/090000_300/imported.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("20260101/import.ics/imported.jsonl"),
            Some(Family::StructuredImport)
        );
        assert_ne!(
            classify("20260101/import.ics/imported.jsonl"),
            Some(Family::AiChat)
        );
        assert_eq!(
            classify("20260101/import.claude/thread_a/conversation_transcript.jsonl"),
            Some(Family::AiChat)
        );
        assert_ne!(
            classify("20260101/import.claude/thread_a/conversation_transcript.jsonl"),
            Some(Family::StructuredImport)
        );
        assert_eq!(
            classify("20260101/import.chatgpt/conv_b/imported_audio.jsonl"),
            Some(Family::AiChat)
        );
        assert_eq!(
            classify("20260508/chat/120000_300/chat.jsonl"),
            Some(Family::Chat)
        );
        assert_ne!(
            classify("20260508/chat/120000_300/chat.jsonl"),
            Some(Family::Browser)
        );
        assert_eq!(
            classify("20260703/suze.browser/000141_317/browser_mail-google-com.jsonl"),
            Some(Family::Browser)
        );
        assert_ne!(
            classify("20260703/suze.browser/000141_317/browser_mail-google-com.jsonl"),
            Some(Family::AiChat)
        );
        assert_ne!(
            classify("20260703/suze.browser/000141_317/browser_mail-google-com.jsonl"),
            Some(Family::Chat)
        );
        assert_eq!(
            classify("facets/work/news/20240101.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("imports/20260101_120000/summary.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("apps/todos/talents/digest.md"),
            Some(Family::Markdown)
        );
        assert_eq!(
            classify("config/actions/20240101.jsonl"),
            Some(Family::ActionLog)
        );
        assert_eq!(
            classify("facets/work/events/20240101.jsonl"),
            Some(Family::Event)
        );
        assert_eq!(
            classify("facets/work/activities/20240101.jsonl"),
            Some(Family::Activity)
        );
        assert_eq!(
            classify("facets/work/logs/20240101.jsonl"),
            Some(Family::ActionLog)
        );
        assert_eq!(classify("notes/foo.txt"), None);
        assert_eq!(
            classify("facets/work/entities/foo.jsonl"),
            Some(Family::FacetEntity)
        );
        assert_eq!(
            classify("facets/work/entities/alice/observations.jsonl"),
            Some(Family::Observation)
        );
        assert_eq!(classify("entities/alice/entity.json"), None);
        assert_eq!(classify("facets/work/entities/alice/entity.json"), None);
        assert_eq!(classify("20240101/default/123456_300/audio.jsonl"), None);
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
                .contains("**Talent:** [123](/app/sol/123)")
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
