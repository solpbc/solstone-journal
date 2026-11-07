// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sense says where it found each entity. A heard source must have been heard.
//!
//! The model picks `source` itself, and a name that was only on screen can come
//! back as `voice` or `transcript`. Rules, applied after generation:
//!
//! 0. A name the segment's input never says or shows is not in this segment,
//!    and is dropped. Sense also reads the owner's known entities in the facet
//!    list, and a name from there alone is the journal's memory, not something
//!    seen or heard now. The check is the lenient heard test below over the
//!    whole input, so a name the model spelled out in full ("Avery Nguyen" for
//!    "AN") stays.
//! 1. With no transcript in its input, nothing was heard: `voice` and
//!    `transcript` become `screen`, or `other` without screen input either.
//!    A `speaker_label` with screen input becomes `screen` too: a participant
//!    label with nothing heard is a name on a call's screen.
//! 2. With a transcript, a heard name that nothing else the model read says,
//!    but a screen input shows, becomes `screen`. The check leans toward heard:
//!    any significant word spelled close enough, accents folded, or the name's
//!    initials count, so a name speech recognition misspelled or shortened
//!    stays heard. Only what the owner saw counts as shown: screen descriptions,
//!    browser pages, window titles and protected-video notes. The owner's own name is
//!    left alone, because a recognized owner voice is labelled "You".
//! 3. One entry per name, type and source: repeats the first two rules
//!    create, or the model wrote, are dropped.
//!
//! 4. A `mentioned` name whose source is `screen` was only seen, so its role
//!    becomes `visible`, unless the input holds no screen at all, or a
//!    transcript in it says the name by the same lenient check: a name both
//!    said and shown stays `mentioned`. Attendees are left alone.
//!
//! After rule 0, nothing is dropped for being unheard; only its source and role change.

use crate::{JournalOwner, NamedActor, PreparedTalent};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

const HEARD: [&str; 2] = ["voice", "transcript"];
const CLOSE_ENOUGH: f64 = 75.0;
const MIN_WORD: usize = 3;
const MIN_PART: usize = 4;
const FILLER: [&str; 6] = ["the", "and", "for", "inc", "llc", "app"];

/// The response with grounded entity sources and roles, or `None` when nothing changed.
pub(crate) fn ground(prepared: &PreparedTalent, journal: &Path, response: &str) -> Option<String> {
    if prepared.name != "sense" {
        return None;
    }
    let mut output = serde_json::from_str::<Value>(response).ok()?;
    let entities = output.get_mut("entities")?.as_array_mut()?;
    let mut changed = false;
    if let Some(input) = prepared
        .config
        .get("transcript")
        .and_then(Value::as_str)
        .filter(|input| !input.trim().is_empty())
    {
        let input = Heard::new(input);
        let before = entities.len();
        entities.retain(|entity| {
            input.named(
                entity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        });
        changed = entities.len() != before;
    }
    let counts = prepared.config.get("source_counts");
    let Some(transcripts) = counts
        .and_then(|counts| counts.get("transcripts"))
        .and_then(Value::as_u64)
    else {
        return changed.then(|| output.to_string());
    };
    let percepts = counts
        .and_then(|counts| counts.get("percepts"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // Evidence and the owner are read only when rules 2 and 4 need them: the
    // owner means reading every entity. Either one missing changes nothing.
    let mut evidence = None;
    let mut owner = None;
    for entity in entities.iter_mut() {
        let Some(row) = entity.as_object_mut() else {
            continue;
        };
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if transcripts == 0
            && percepts > 0
            && row.get("source").and_then(Value::as_str) == Some("speaker_label")
        {
            row.insert("source".to_owned(), Value::String("screen".to_owned()));
            changed = true;
        }
        if !row
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| HEARD.contains(&source))
        {
            if seen_only(row)
                && if transcripts == 0 {
                    percepts > 0
                } else {
                    evidence
                        .get_or_insert_with(|| Evidence::read(prepared))
                        .as_ref()
                        .is_some_and(|evidence| {
                            !evidence.shown.is_empty() && !evidence.was_heard(&name)
                        })
                }
            {
                row.insert("role".to_owned(), Value::String("visible".to_owned()));
                changed = true;
            }
            continue;
        }
        let grounded = if transcripts == 0 {
            Some(if percepts > 0 { "screen" } else { "other" })
        } else {
            evidence
                .get_or_insert_with(|| Evidence::read(prepared))
                .as_ref()
                .filter(|evidence| !evidence.was_heard(&name) && evidence.was_shown(&name))
                .filter(|_| {
                    owner
                        .get_or_insert_with(|| JournalOwner::load(journal).ok())
                        .as_ref()
                        .is_some_and(|owner| !names_owner(owner, &name))
                })
                .map(|_| "screen")
        };
        if let Some(source) = grounded {
            row.insert("source".to_owned(), Value::String(source.to_owned()));
            if seen_only(row) {
                row.insert("role".to_owned(), Value::String("visible".to_owned()));
            }
            changed = true;
        }
    }
    let mut seen = BTreeSet::new();
    let before = entities.len();
    entities.retain(|entity| {
        let field = |key| entity.get(key).and_then(Value::as_str).unwrap_or_default();
        seen.insert((
            crate::fold_name(field("name")),
            field("type").to_owned(),
            field("source").to_owned(),
        ))
    });
    changed |= entities.len() != before;
    changed.then(|| output.to_string())
}

/// A `mentioned` row from the screen.
fn seen_only(row: &serde_json::Map<String, Value>) -> bool {
    let field = |key| row.get(key).and_then(Value::as_str).unwrap_or_default();
    field("role") == "mentioned" && field("source") == "screen"
}

/// The owner, or a shortening of one of the owner's names ("Kat", "Kathryn M").
fn names_owner(owner: &JournalOwner, name: &str) -> bool {
    if owner.actor(name) == NamedActor::Owner {
        return true;
    }
    let parts = words(name);
    !parts.is_empty()
        && owner.names.iter().any(|known| {
            let known = words(known);
            parts
                .iter()
                .all(|part| known.iter().any(|word| word.starts_with(part.as_str())))
        })
}

/// What the model read, split into what the owner saw and everything else.
struct Evidence {
    heard: Heard,
    shown: String,
}

/// Text that was heard, and the lenient test of whether a name was said in it.
pub(crate) struct Heard {
    text: String,
    words: BTreeSet<String>,
}

impl Heard {
    pub(crate) fn new(text: &str) -> Self {
        let heard = words(text);
        let mut words = heard.iter().cloned().collect::<BTreeSet<_>>();
        words.extend(heard.windows(2).map(|pair| pair.concat()));
        Self {
            text: heard.concat(),
            words,
        }
    }

    /// The check leans toward heard: the whole name, its initials, or any
    /// significant word spelled close enough, accents folded.
    pub(crate) fn said(&self, name: &str) -> bool {
        let name_words = words(name);
        let significant = name_words
            .iter()
            .filter(|word| word.chars().count() >= MIN_WORD && !FILLER.contains(&word.as_str()))
            .collect::<Vec<_>>();
        if significant.is_empty() || self.text.contains(&name_words.concat()) {
            return true;
        }
        let initials = name_words
            .iter()
            .filter_map(|word| word.chars().next())
            .collect::<String>();
        if name_words.len() > 1 && self.words.contains(&initials) {
            return true;
        }
        significant.into_iter().any(|word| {
            let length = word.chars().count();
            self.words.contains(word)
                || self.words.iter().any(|heard| {
                    // Two lengths this far apart cannot score close enough.
                    let other = heard.chars().count();
                    2.0 * length.min(other) as f64 * 100.0 >= CLOSE_ENOUGH * (length + other) as f64
                        && solstone_core_entity_matching::token_sort_ratio(word, heard)
                            >= CLOSE_ENOUGH
                })
        })
    }
}

impl Heard {
    /// Rule 0's test: said, or a word of the name and a word of the input
    /// where one holds the other ("Homebrew" for `brew`), at least
    /// `MIN_PART` letters long.
    fn named(&self, name: &str) -> bool {
        self.said(name)
            || words(name)
                .iter()
                .filter(|word| word.chars().count() >= MIN_PART)
                .any(|word| {
                    self.words.iter().any(|seen| {
                        seen.chars().count() >= MIN_PART
                            && (word.contains(seen.as_str()) || seen.contains(word.as_str()))
                    })
                })
    }
}

impl Evidence {
    /// Shown text is every seen input, cut from the input at its rendered
    /// range. Everything else the model read counts as heard, unsourced and
    /// unrecognized inputs included, so an input the runtime cannot classify
    /// never makes a name look unheard.
    fn read(prepared: &PreparedTalent) -> Option<Self> {
        let input = prepared.config.get("transcript")?.as_str()?;
        let shift = if input.starts_with(crate::prepare::SPARSE_INPUT_NOTE) {
            crate::prepare::SPARSE_INPUT_NOTE.len()
        } else {
            0
        };
        let mut cuts = Vec::new();
        let mut shown = Vec::new();
        for source in prepared.entry_sources.iter().flatten() {
            if !solstone_core_transcripts::is_seen_input(&source.file) {
                continue;
            }
            let range = source.rendered_range.start.saturating_add(shift)
                ..source.rendered_range.end.saturating_add(shift);
            if input.get(range.clone()).is_some() {
                shown.extend(source.chunks.iter().flat_map(|(chunk, _)| words(chunk)));
                cuts.push(range);
            }
        }
        cuts.sort_by_key(|range| range.start);
        let mut rest = String::with_capacity(input.len());
        let mut at = 0;
        for range in cuts {
            if range.start >= at {
                rest.push_str(&input[at..range.start]);
                rest.push('\n');
                at = range.end;
            }
        }
        rest.push_str(&input[at..]);
        Some(Self {
            heard: Heard::new(&rest),
            shown: shown.concat(),
        })
    }

    fn was_heard(&self, name: &str) -> bool {
        self.heard.said(name)
    }

    fn was_shown(&self, name: &str) -> bool {
        let name = words(name).concat();
        !name.is_empty() && self.shown.contains(&name)
    }
}

/// Lowercase words with accents folded, so "José" and "Jose" are one word.
fn words(text: &str) -> Vec<String> {
    text.nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};
    use solstone_core_transcripts::EntrySource;

    fn source(file: &str, lines: &[&str]) -> (Option<EntrySource>, &'static str) {
        (
            Some(EntrySource {
                path: file.into(),
                sha256: "fixture".into(),
                rendered_range: 0..0,
                stream: None,
                file: file.into(),
                records: Vec::new(),
                unparsed_lines: 0,
                chunks: lines
                    .iter()
                    .map(|line| ((*line).to_owned(), Map::new()))
                    .collect(),
                chunk_offsets: Vec::new(),
                voice_named: BTreeSet::new(),
            }),
            "",
        )
    }

    /// A Sense input assembled from `sources`; a `None` source renders `text`.
    fn sense(
        transcripts: usize,
        percepts: usize,
        sources: Vec<(Option<EntrySource>, &str)>,
    ) -> PreparedTalent {
        let mut sources = sources;
        let mut transcript = String::new();
        for (source, text) in &mut sources {
            transcript.push_str("### next\n");
            let body = match source {
                Some(source) => source
                    .chunks
                    .iter()
                    .map(|(chunk, _)| chunk.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                None => (*text).to_owned(),
            };
            if let Some(source) = source {
                source.rendered_range = transcript.len()..transcript.len() + body.len();
            }
            transcript.push_str(&body);
            transcript.push('\n');
        }
        let mut prepared = PreparedTalent::new(
            "sense".into(),
            json!({"transcript":transcript,"source_counts":{"transcripts":transcripts,"percepts":percepts,"talents":0}})
                .as_object()
                .unwrap()
                .clone(),
        );
        prepared.entry_sources = sources.into_iter().map(|(source, _)| source).collect();
        prepared
    }

    /// A journal with an empty entity store, so the owner loads.
    fn journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("entities")).unwrap();
        journal
    }

    fn entity(name: &str, kind: &str, source: &str) -> Value {
        json!({"type":kind,"name":name,"role":"mentioned","source":source,"level":"low","context":"came up"})
    }

    fn grounded(prepared: &PreparedTalent, entities: Vec<Value>) -> Vec<(String, String)> {
        let journal = journal();
        let response = json!({"density":"active","entities":entities}).to_string();
        let output = ground(prepared, journal.path(), &response).unwrap_or(response);
        serde_json::from_str::<Value>(&output).unwrap()["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().into(),
                    e["source"].as_str().unwrap().into(),
                )
            })
            .collect()
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect()
    }

    #[test]
    fn a_name_only_in_the_facet_list_is_not_in_the_segment() {
        // The owner's journal knows a film from an earlier week. Sense reads it in
        // the facet list, but this terminal segment never shows it.
        let terminal = sense(
            0,
            1,
            vec![source(
                "tmux_0_screen.jsonl",
                &[
                    "~/src/harborlight $ cargo test parse",
                    "test result: ok. 2 passed",
                ],
            )],
        );
        assert_eq!(
            grounded(
                &terminal,
                vec![
                    entity("Dogma", "Project", "screen"),
                    entity("Kevin Smith", "Person", "transcript"),
                    entity("harborlight", "Project", "screen"),
                ]
            ),
            pairs(&[("harborlight", "screen")])
        );
        let speech = sense(
            1,
            0,
            vec![(None, "Speaker 1: the tide table parser is fixed now")],
        );
        assert_eq!(
            grounded(&speech, vec![entity("Dogma", "Project", "voice")]),
            pairs(&[])
        );
    }

    #[test]
    fn a_name_the_model_spelled_out_from_the_segment_stays() {
        let terminal = sense(
            0,
            1,
            vec![source(
                "tmux_0_screen.jsonl",
                &[
                    "~ $ brew upgrade",
                    "notes: rewatch with Sam, ask AN about the deck",
                ],
            )],
        );
        assert_eq!(
            grounded(
                &terminal,
                vec![
                    entity("Homebrew", "Tool", "screen"),
                    entity("Sam Ortiz", "Person", "screen"),
                    entity("Avery Nguyen", "Person", "screen"),
                ]
            ),
            pairs(&[
                ("Homebrew", "screen"),
                ("Sam Ortiz", "screen"),
                ("Avery Nguyen", "screen")
            ])
        );
    }

    #[test]
    fn a_segment_without_a_transcript_hears_no_one() {
        let screen = sense(
            0,
            1,
            vec![source(
                "tmux_1_screen.jsonl",
                &["Ada Lovelace reviewed the LyricWhiz paper with Grace Hopper"],
            )],
        );
        assert_eq!(
            grounded(
                &screen,
                vec![
                    entity("Ada Lovelace", "Person", "voice"),
                    entity("LyricWhiz", "Project", "transcript"),
                    entity("Grace Hopper", "Person", "speaker_label"),
                ]
            ),
            pairs(&[
                ("Ada Lovelace", "screen"),
                ("LyricWhiz", "screen"),
                ("Grace Hopper", "screen")
            ])
        );
        let nothing = sense(0, 0, Vec::new());
        assert_eq!(
            grounded(
                &nothing,
                vec![
                    entity("Ada Lovelace", "Person", "voice"),
                    entity("Grace Hopper", "Person", "speaker_label"),
                ]
            ),
            pairs(&[("Ada Lovelace", "other"), ("Grace Hopper", "speaker_label")])
        );
    }

    #[test]
    fn a_call_on_screen_with_nothing_heard_keeps_its_attendees_seen() {
        // A phone screen recording of a video call, with no audio: the names
        // on the call's tiles are attendees, read from the screen.
        let call = sense(0, 1, vec![source("screen.jsonl", &["Aaron Daniel jc"])]);
        let journal = journal();
        let response = json!({"entities":[
            {"type":"Person","name":"Aaron","role":"attendee","source":"speaker_label","level":"high","context":"on a tile"}
        ]})
        .to_string();
        let output = ground(&call, journal.path(), &response).expect("grounded");
        let row = &serde_json::from_str::<Value>(&output).unwrap()["entities"][0];
        assert_eq!(
            (row["role"].as_str(), row["source"].as_str()),
            (Some("attendee"), Some("screen"))
        );

        // With a transcript, a speaker label may be the transcript's own.
        let heard = sense(1, 1, vec![(None, "Aaron: hi all")]);
        assert_eq!(
            grounded(&heard, vec![entity("Aaron", "Person", "speaker_label")]),
            pairs(&[("Aaron", "speaker_label")])
        );
    }

    #[test]
    fn a_name_only_on_screen_is_not_heard_and_appears_once() {
        // The reported shape: one screen name written three times, three ways.
        let prepared = sense(
            1,
            1,
            vec![
                source(
                    "audio.jsonl",
                    &["[22:05:01] Speaker 1: let's look at the lyrics results"],
                ),
                source(
                    "display_1_screen.jsonl",
                    &["Assistant: LyricWhiz aligns lyrics to audio"],
                ),
            ],
        );
        assert_eq!(
            grounded(
                &prepared,
                vec![
                    entity("LyricWhiz", "Project", "screen"),
                    entity("LyricWhiz", "Project", "transcript"),
                    entity("LyricWhiz", "Project", "voice"),
                ]
            ),
            pairs(&[("LyricWhiz", "screen")])
        );
    }

    #[test]
    fn names_that_were_said_keep_their_heard_source() {
        let prepared = sense(
            1,
            1,
            vec![
                source(
                    "audio.jsonl",
                    &[
                        "[09:00:01] Speaker 1: I asked Maya about the soul stone release",
                        "[09:00:04] Ada Lovelace: and Kubernets is down again",
                    ],
                ),
                source(
                    "display_1_screen.jsonl",
                    &["Maya Chen · Solstone · Kubernetes · Ada Lovelace · Priya Patel"],
                ),
            ],
        );
        assert_eq!(
            grounded(
                &prepared,
                vec![
                    entity("Maya Chen", "Person", "voice"),
                    entity("Solstone", "Project", "voice"),
                    entity("Kubernetes", "Tool", "transcript"),
                    entity("Ada Lovelace", "Person", "voice"),
                    entity("Priya Patel", "Person", "voice"),
                ]
            ),
            pairs(&[
                ("Maya Chen", "voice"),
                ("Solstone", "voice"),
                ("Kubernetes", "transcript"),
                ("Ada Lovelace", "voice"),
                ("Priya Patel", "screen"),
            ])
        );
    }

    #[test]
    fn screen_text_that_repeats_a_spoken_word_leaves_the_speech_heard() {
        let prepared = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[09:00:01] Speaker 1: Maya"]),
                source("display_1_screen.jsonl", &["Maya"]),
            ],
        );
        assert_eq!(
            grounded(&prepared, vec![entity("Maya", "Person", "voice")]),
            pairs(&[("Maya", "voice")])
        );
    }

    #[test]
    fn any_input_that_is_not_a_screen_counts_as_heard() {
        // A recording transcribed under its own name, found by its header.
        let prepared = sense(
            1,
            1,
            vec![
                source(
                    "meeting.jsonl",
                    &["[09:00:01] Speaker 1: Priya Patel joined"],
                ),
                source("display_1_screen.jsonl", &["Priya Patel · Calendar"]),
            ],
        );
        assert_eq!(
            grounded(&prepared, vec![entity("Priya Patel", "Person", "voice")]),
            pairs(&[("Priya Patel", "voice")])
        );
    }

    #[test]
    fn accents_initials_and_short_names_stay_heard() {
        let prepared = sense(
            1,
            1,
            vec![
                source(
                    "audio.jsonl",
                    &["[09:00:01] Speaker 1: Jose and AN said Steven would call"],
                ),
                source(
                    "display_1_screen.jsonl",
                    &["José · Avery Nguyen · Bo Li · Stephen Hale · Grace Hopper"],
                ),
            ],
        );
        assert_eq!(
            grounded(
                &prepared,
                vec![
                    entity("José", "Person", "voice"),
                    entity("Avery Nguyen", "Person", "voice"),
                    entity("Bo Li", "Person", "voice"),
                    entity("Stephen Hale", "Person", "voice"),
                    entity("Grace Hopper", "Person", "voice"),
                ]
            ),
            pairs(&[
                ("José", "voice"),
                ("Avery Nguyen", "voice"),
                ("Bo Li", "voice"),
                ("Stephen Hale", "voice"),
                ("Grace Hopper", "screen"),
            ])
        );
    }

    #[test]
    fn repeats_are_the_same_name_not_the_same_letters() {
        let prepared = sense(
            0,
            1,
            vec![source("display_1_screen.jsonl", &["C · C++ · C#"])],
        );
        assert_eq!(
            grounded(
                &prepared,
                vec![
                    entity("C", "Tool", "screen"),
                    entity("C++", "Tool", "screen"),
                    entity("C#", "Tool", "voice"),
                    entity("c ", "Tool", "screen"),
                ]
            ),
            pairs(&[("C", "screen"), ("C++", "screen"), ("C#", "screen")])
        );
    }

    #[test]
    fn a_name_neither_heard_nor_shown_is_not_in_the_segment() {
        let prepared = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[09:00:01] Speaker 1: the pump is fixed"]),
                source("display_1_screen.jsonl", &["Irrigation dashboard"]),
            ],
        );
        let journal = journal();
        let response = json!({"entities":[entity("Maya Chen", "Person", "voice")]}).to_string();
        assert_eq!(
            ground(&prepared, journal.path(), &response).as_deref(),
            Some(r#"{"entities":[]}"#)
        );
    }

    #[test]
    fn an_unsourced_input_counts_as_heard_and_never_hides_the_screen() {
        // Browser and desktop-activity inputs carry no typed source: what they
        // say may have been heard, and a name on screen beside them is still shown.
        let prepared = sense(
            1,
            2,
            vec![
                source(
                    "audio.jsonl",
                    &["[22:05:01] Speaker 1: let's compare the results"],
                ),
                (None, "Window: Ada Lovelace notes"),
                source(
                    "display_1_screen.jsonl",
                    &["Ada Lovelace · LyricWhiz · Grace Hopper"],
                ),
            ],
        );
        assert_eq!(
            grounded(
                &prepared,
                vec![
                    entity("Ada Lovelace", "Person", "voice"),
                    entity("LyricWhiz", "Project", "voice"),
                ]
            ),
            pairs(&[("Ada Lovelace", "voice"), ("LyricWhiz", "screen")])
        );
        let journal = journal();
        let mut unread = prepared.clone();
        unread.config.remove("transcript");
        let response = json!({"entities":[entity("LyricWhiz", "Project", "voice")]}).to_string();
        assert_eq!(ground(&unread, journal.path(), &response), None);
    }

    #[test]
    fn the_owners_name_keeps_its_voice() {
        let journal = journal();
        let owner = journal.path().join("config");
        std::fs::create_dir_all(&owner).unwrap();
        std::fs::write(
            owner.join("journal.json"),
            r#"{"identity":{"name":"Avery Nguyen"}}"#,
        )
        .unwrap();
        let prepared = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[09:00:01] You: I'll send it tonight"]),
                source(
                    "display_1_screen.jsonl",
                    &["Avery Nguyen · Inbox · Priya Patel"],
                ),
            ],
        );
        let response = json!({"entities":[
            entity("Avery Nguyen", "Person", "voice"),
            entity("Priya Patel", "Person", "voice"),
            entity("Avery N", "Person", "voice"),
        ]})
        .to_string();
        let output: Value =
            serde_json::from_str(&ground(&prepared, journal.path(), &response).unwrap()).unwrap();
        assert_eq!(output["entities"][0]["source"], "voice");
        assert_eq!(output["entities"][1]["source"], "screen");
        assert_eq!(output["entities"][2]["source"], "voice");
    }
    fn roles(prepared: &PreparedTalent, entities: Vec<Value>) -> Vec<(String, String, String)> {
        let journal = journal();
        let response = json!({"density":"active","entities":entities}).to_string();
        let output = ground(prepared, journal.path(), &response).unwrap_or(response);
        serde_json::from_str::<Value>(&output).unwrap()["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["name"].as_str().unwrap().into(),
                    e["role"].as_str().unwrap().into(),
                    e["source"].as_str().unwrap().into(),
                )
            })
            .collect()
    }

    fn with_role(name: &str, role: &str, source: &str) -> Value {
        json!({"type":"Person","name":name,"role":role,"source":source,"level":"low","context":"came up"})
    }

    fn triples(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
        rows.iter()
            .map(|(a, b, c)| ((*a).into(), (*b).into(), (*c).into()))
            .collect()
    }

    #[test]
    fn a_name_only_on_screen_is_visible_not_mentioned() {
        // #404 case 2: paper authors from an on-screen literature search.
        let terminal = sense(
            0,
            1,
            vec![source(
                "tmux_1_screen.jsonl",
                &["Cifka and Suda, 2024", "call: Grace Hopper"],
            )],
        );
        assert_eq!(
            roles(
                &terminal,
                vec![
                    with_role("Cifka", "mentioned", "voice"),
                    with_role("Suda", "mentioned", "screen"),
                    with_role("Grace Hopper", "attendee", "speaker_label"),
                ]
            ),
            triples(&[
                ("Cifka", "visible", "screen"),
                ("Suda", "visible", "screen"),
                ("Grace Hopper", "attendee", "screen"),
            ])
        );
    }

    #[test]
    fn a_name_that_was_said_stays_mentioned_beside_the_screen() {
        let mixed = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[10:00:01] Speaker 1: Ada sent the draft"]),
                source(
                    "desktop_screen.jsonl",
                    &["Ada Lovelace — draft.pdf", "Cifka et al."],
                ),
            ],
        );
        assert_eq!(
            roles(
                &mixed,
                vec![
                    with_role("Ada Lovelace", "mentioned", "voice"),
                    with_role("Cifka", "mentioned", "transcript"),
                ]
            ),
            triples(&[
                ("Ada Lovelace", "mentioned", "voice"),
                ("Cifka", "visible", "screen"),
            ])
        );
    }

    #[test]
    fn a_screen_name_that_was_said_stays_mentioned() {
        let mixed = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[10:00:01] Speaker 1: ask Ada about it"]),
                source("desktop_screen.jsonl", &["Ada", "Suda"]),
            ],
        );
        assert_eq!(
            roles(
                &mixed,
                vec![
                    with_role("Ada", "mentioned", "screen"),
                    with_role("Suda", "mentioned", "screen")
                ]
            ),
            triples(&[
                ("Ada", "mentioned", "screen"),
                ("Suda", "visible", "screen")
            ])
        );
        // Without screen input nothing is visible, whatever source the model wrote.
        let neither = sense(0, 0, vec![]);
        assert_eq!(
            roles(
                &neither,
                vec![
                    with_role("Ada", "mentioned", "voice"),
                    with_role("Suda", "mentioned", "screen")
                ]
            ),
            triples(&[
                ("Ada", "mentioned", "other"),
                ("Suda", "mentioned", "screen")
            ])
        );
        let audio = sense(
            1,
            0,
            vec![source(
                "audio.jsonl",
                &["[10:00:01] Speaker 1: hello from Suda"],
            )],
        );
        assert_eq!(
            roles(&audio, vec![with_role("Suda", "mentioned", "screen")]),
            triples(&[("Suda", "mentioned", "screen")])
        );
    }

    #[test]
    fn a_name_in_a_window_title_was_seen_not_heard() {
        // #404: a shop named after a person, open in front while audio ran.
        // The window title names her; nothing heard does.
        let desktop = sense(
            1,
            1,
            vec![
                source("audio.jsonl", &["[10:00:01] Speaker 1: that print is nice"]),
                source(
                    "090000_300_desktop_activity.jsonl",
                    &["Firefox — Mara Quinn Prints"],
                ),
            ],
        );
        assert_eq!(
            roles(
                &desktop,
                vec![
                    with_role("Mara Quinn", "mentioned", "screen"),
                    with_role("Mara Quinn", "mentioned", "voice"),
                ]
            ),
            triples(&[("Mara Quinn", "visible", "screen")])
        );
    }
}
