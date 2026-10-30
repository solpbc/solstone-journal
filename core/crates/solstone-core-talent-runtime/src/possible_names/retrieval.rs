// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which registry entities a segment's speech may name, before the model is
//! asked. Every rule is a port of the evaluated prototype (pre v2 and its
//! `g1` gate); the thresholds live in [`super::policy`].
//!
//! Routes, by priority: `canonical` (span equals the name), `alias` (equals a
//! recorded alias), `first` (one word equal to the first name of a person),
//! `phonetic` (same sound key, different spelling), `spelling` (close
//! spelling). Each entity keeps one action per segment: its best route,
//! preferring a capitalized mid-sentence occurrence, then the earliest. Every
//! other entity matched on the same span is a competitor.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use solstone_core_possible_names::CandidateEntity;
use unicode_normalization::UnicodeNormalization;

use super::phon;
use super::policy::*;

/// A word: letters and digits, joined by an apostrophe or hyphen. Python's
/// Unicode `[^\W_]` is letters and numbers; Rust's `\w` would add marks.
static WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{L}\p{N}]+(?:['’\-][\p{L}\p{N}]+)*").expect("fixed word pattern")
});
/// A character a multi-word span may not contain: anything but a word
/// character, whitespace, an apostrophe or a hyphen. `\x1c`-`\x1f` are
/// whitespace to Python, not to Unicode.
static OUTSIDE_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}_\s\x{1c}-\x{1f}'’\-]").expect("fixed span pattern"));
static NAME_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}_]+").expect("fixed token pattern"));
static NOT_ALNUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]").expect("fixed alphanumeric pattern"));

/// How a span matched an entity, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Route {
    Canonical,
    Alias,
    First,
    Phonetic,
    Spelling,
}

impl Route {
    pub(crate) fn priority(self) -> u8 {
        self as u8
    }

    /// What the model is told about the match.
    pub(crate) fn basis(self) -> &'static str {
        match self {
            Self::Canonical => "same normalized spelling as the name",
            Self::Alias => "same normalized spelling as a known alias",
            Self::First => "first name of a person in the registry",
            Self::Phonetic => "sounds like the name",
            Self::Spelling => "close spelling of the name",
        }
    }

    pub(crate) fn enabled(self) -> bool {
        match self {
            Self::Canonical => ROUTE_CANONICAL,
            Self::Alias => ROUTE_ALIAS,
            Self::First => ROUTE_FIRST,
            Self::Phonetic => ROUTE_PHONETIC,
            Self::Spelling => ROUTE_SPELLING,
        }
    }
}

/// One transcript statement: its 1-based ordinal and the words readers show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Statement {
    pub sentence: u64,
    pub text: String,
}

/// The best match of one entity in one segment, with the features the gate
/// reads.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Action {
    pub entity_id: String,
    /// 1-based statement ordinal.
    pub sentence: u64,
    /// UTF-8 byte span in the statement's text.
    pub start_byte: usize,
    pub end_byte: usize,
    /// The words at the span.
    pub heard: String,
    pub matched_form: String,
    pub route: Route,
    pub words: usize,
    pub cap: bool,
    pub sentence_start: bool,
    pub all_words_cap: bool,
    /// Other entities matched on the same span, by id.
    pub competitors: Vec<String>,
    pub product: bool,
    /// Every word of the name already occurs in the segment's derived text.
    pub in_segment_derived: bool,
    /// The entity is filed under one of the segment's own facets.
    pub segment_facet: bool,
}

impl Action {
    /// Why the gate drops this action, or `None` when it is kept.
    pub(crate) fn drop_reason(&self) -> Option<&'static str> {
        if self.product {
            return Some("product");
        }
        if !(self.cap && !self.sentence_start) {
            return Some("not_capitalized_mid_sentence");
        }
        if matches!(self.route, Route::Phonetic | Route::Spelling) && !self.all_words_cap {
            return Some("not_every_word_capitalized");
        }
        if self.route == Route::Canonical && self.in_segment_derived {
            return Some("already_in_derived_text");
        }
        if self.route == Route::First && self.competitors.len() > FIRST_MAX_COMPETITORS {
            return Some("first_name_rivals");
        }
        if !self.route.enabled() {
            return Some("route_disabled");
        }
        if ALIAS_NEEDS_SEGMENT_FACET && self.route == Route::Alias && !self.segment_facet {
            return Some("alias_outside_segment_facets");
        }
        None
    }
}

/// NFKC, full case folding, letters and numbers only: the match key.
pub(crate) fn compact(text: &str) -> String {
    let folded = caseless::default_case_fold_str(&text.nfkc().collect::<String>());
    NOT_ALNUM.replace_all(&folded, "").into_owned()
}

/// Whether two written names are near spellings of each other, by the rule
/// the spelling route matches a span to a name with: the same first letter and
/// word count, and match keys of [`MIN_SPELLING_CHARS`] or more that differ,
/// are within [`SPELLING_MAX_LENGTH_DIFFERENCE`] characters in length and reach
/// [`SPELLING_MIN_RATIO`].
pub(crate) fn near_spelling(left: &str, right: &str) -> bool {
    let (left_key, right_key) = (compact(left), compact(right));
    let (left_length, right_length) = (char_count(&left_key), char_count(&right_key));
    left_key != right_key
        && left_length >= MIN_SPELLING_CHARS
        && right_length >= MIN_SPELLING_CHARS
        && left_key.chars().next() == right_key.chars().next()
        && split_words(left).count() == split_words(right).count()
        && left_length.abs_diff(right_length) <= SPELLING_MAX_LENGTH_DIFFERENCE
        && rapidfuzz::fuzz::ratio(left_key.chars(), right_key.chars()) * 100.0 >= SPELLING_MIN_RATIO
}

/// Python's `str.casefold`.
pub(crate) fn casefold(text: &str) -> String {
    caseless::default_case_fold_str(text)
}

/// Python's `str.isspace` for one character.
fn is_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// Python's `str.split()`.
fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_space).filter(|word| !word.is_empty())
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn starts_uppercase(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_uppercase)
}

/// Whether a name is one of sol pbc's own product names, which the product
/// spelling fix already handles: the evaluated set, plus anything the fix's
/// spelling set rewrites to one of them.
pub(crate) fn is_product(name: &str) -> bool {
    PRODUCT_NAME_KEYS.contains(&compact(name).as_str())
        || PRODUCT_NAME_KEYS
            .contains(&compact(&crate::product_names::canonical_spelling(name)).as_str())
}

/// Whether every word of `name` is a substring of `derived`, which is already
/// case folded. A name without words counts as present.
pub(crate) fn in_derived_text(name: &str, derived: &str) -> bool {
    NAME_TOKEN
        .find_iter(name)
        .all(|token| derived.contains(&casefold(token.as_str())))
}

struct Keys {
    exact: HashMap<String, Vec<(usize, Route, String)>>,
    first: HashMap<String, Vec<(usize, String)>>,
    sound: HashMap<String, Vec<(usize, String, String, usize)>>,
    spelling: HashMap<char, Vec<(String, usize, String, usize)>>,
}

fn keys(entities: &[CandidateEntity]) -> Keys {
    let mut keys = Keys {
        exact: HashMap::new(),
        first: HashMap::new(),
        sound: HashMap::new(),
        spelling: HashMap::new(),
    };
    for (index, entity) in entities.iter().enumerate() {
        let forms = std::iter::once((entity.name.as_str(), Route::Canonical))
            .chain(entity.aka.iter().map(|aka| (aka.as_str(), Route::Alias)));
        let mut seen = BTreeSet::new();
        for (form, route) in forms {
            let key = compact(form);
            let length = char_count(&key);
            if !(MIN_FORM_CHARS..=MAX_FORM_CHARS).contains(&length) || !seen.insert(key.clone()) {
                continue;
            }
            keys.exact
                .entry(key.clone())
                .or_default()
                .push((index, route, form.to_owned()));
            let words = split_words(form).count();
            let sound = phon::key(form);
            if char_count(&sound) >= MIN_SOUND_KEY_CHARS {
                keys.sound.entry(sound).or_default().push((
                    index,
                    form.to_owned(),
                    key.clone(),
                    words,
                ));
            }
            if length >= MIN_SPELLING_CHARS {
                let initial = key.chars().next().expect("non-empty key");
                keys.spelling.entry(initial).or_default().push((
                    key,
                    index,
                    form.to_owned(),
                    words,
                ));
            }
        }
        let words = split_words(&entity.name).collect::<Vec<_>>();
        if entity.entity_type == FIRST_NAME_TYPE
            && words.len() >= MIN_FIRST_NAME_WORDS
            && char_count(&compact(words[0])) >= MIN_FIRST_NAME_CHARS
        {
            keys.first
                .entry(compact(words[0]))
                .or_default()
                .push((index, words[0].to_owned()));
        }
    }
    keys
}

/// Every entity matched in the segment, one action each, before the gate.
/// `derived` is the segment's derived text, case folded; `segment_facets`
/// are the facets the segment is filed under.
pub(crate) fn find_actions(
    statements: &[Statement],
    entities: &[CandidateEntity],
    derived: &str,
    segment_facets: &[String],
) -> Vec<Action> {
    type Rank = (u8, bool, u64, usize);
    let keys = keys(entities);
    let mut best = HashMap::<usize, (Rank, Action)>::new();
    for statement in statements {
        let text = statement.text.as_str();
        let words = WORD
            .find_iter(text)
            .map(|word| (word.start(), word.end()))
            .collect::<Vec<_>>();
        for (first_word, &(start, _)) in words.iter().enumerate() {
            let before = text[..start].trim_end_matches(is_space);
            let sentence_start = before.is_empty() || before.ends_with(['.', '!', '?']);
            for count in 1..=MAX_SPAN_WORDS {
                let Some(&(_, end)) = words.get(first_word + count - 1) else {
                    break;
                };
                let heard = &text[start..end];
                if heard.contains('\n') || (count > 1 && OUTSIDE_SPAN.is_match(heard)) {
                    break;
                }
                let key = compact(heard);
                if key.is_empty() {
                    continue;
                }
                let length = char_count(&key);
                let cap = starts_uppercase(heard);
                let mut hits = Vec::<(usize, Route, &str)>::new();
                for (index, route, form) in keys.exact.get(&key).into_iter().flatten() {
                    hits.push((*index, *route, form));
                }
                if count == 1 {
                    for (index, form) in keys.first.get(&key).into_iter().flatten() {
                        hits.push((*index, Route::First, form));
                    }
                }
                if cap && length >= MIN_SOUND_SPAN_CHARS {
                    let sound = phon::key(heard);
                    for (index, form, form_key, form_words) in
                        keys.sound.get(&sound).into_iter().flatten()
                    {
                        let ratio = length as f64 / char_count(form_key) as f64;
                        if *form_key != key
                            && *form_words == count
                            && (PHONETIC_MIN_LENGTH_RATIO..=PHONETIC_MAX_LENGTH_RATIO)
                                .contains(&ratio)
                        {
                            hits.push((*index, Route::Phonetic, form));
                        }
                    }
                    if length >= MIN_SPELLING_CHARS {
                        let initial = key.chars().next().expect("non-empty key");
                        for (form_key, index, form, form_words) in
                            keys.spelling.get(&initial).into_iter().flatten()
                        {
                            if *form_key != key
                                && *form_words == count
                                && char_count(form_key).abs_diff(length)
                                    <= SPELLING_MAX_LENGTH_DIFFERENCE
                                && rapidfuzz::fuzz::ratio(form_key.chars(), key.chars()) * 100.0
                                    >= SPELLING_MIN_RATIO
                            {
                                hits.push((*index, Route::Spelling, form));
                            }
                        }
                    }
                }
                if hits.is_empty() {
                    continue;
                }
                let matched = hits
                    .iter()
                    .map(|(index, _, _)| entities[*index].entity_id.as_str())
                    .collect::<BTreeSet<_>>();
                let all_words_cap = WORD
                    .find_iter(heard)
                    .all(|word| starts_uppercase(word.as_str()));
                let start_byte = start;
                for (index, route, form) in hits {
                    let entity = &entities[index];
                    let rank = (
                        route.priority(),
                        !(cap && !sentence_start),
                        statement.sentence,
                        start_byte,
                    );
                    if best
                        .get(&index)
                        .is_some_and(|(current, _)| *current <= rank)
                    {
                        continue;
                    }
                    let action = Action {
                        entity_id: entity.entity_id.clone(),
                        sentence: statement.sentence,
                        start_byte,
                        end_byte: end,
                        heard: heard.to_owned(),
                        matched_form: form.to_owned(),
                        route,
                        words: count,
                        cap,
                        sentence_start,
                        all_words_cap,
                        competitors: matched
                            .iter()
                            .filter(|id| **id != entity.entity_id)
                            .map(|id| (*id).to_owned())
                            .collect(),
                        product: is_product(&entity.name),
                        in_segment_derived: false,
                        segment_facet: false,
                    };
                    best.insert(index, (rank, action));
                }
            }
        }
    }
    let mut actions = best
        .into_iter()
        .map(|(index, (_, mut action))| {
            action.in_segment_derived = in_derived_text(&entities[index].name, derived);
            action.segment_facet = segment_facets
                .iter()
                .any(|facet| entities[index].facets.contains_key(facet));
            action
        })
        .collect::<Vec<_>>();
    actions.sort_by(|left, right| {
        (left.sentence, left.start_byte, &left.entity_id).cmp(&(
            right.sentence,
            right.start_byte,
            &right.entity_id,
        ))
    });
    actions
}

/// The actions the gate keeps, capped and in presentation order.
pub(crate) fn select(actions: &[Action]) -> Vec<Action> {
    let mut kept = actions
        .iter()
        .filter(|action| action.drop_reason().is_none())
        .cloned()
        .collect::<Vec<_>>();
    kept.sort_by(|left, right| {
        (
            left.route.priority(),
            left.competitors.len(),
            left.start_byte,
            &left.entity_id,
        )
            .cmp(&(
                right.route.priority(),
                right.competitors.len(),
                right.start_byte,
                &right.entity_id,
            ))
    });
    kept.truncate(MAX_CANDIDATES);
    kept.sort_by(|left, right| {
        (
            left.sentence,
            left.start_byte,
            left.route.priority(),
            &left.entity_id,
        )
            .cmp(&(
                right.sentence,
                right.start_byte,
                right.route.priority(),
                &right.entity_id,
            ))
    });
    kept
}
