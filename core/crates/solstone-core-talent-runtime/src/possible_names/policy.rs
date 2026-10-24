// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Every retrieval and gate threshold of the possible-names talent, as
//! evaluated. Changing one changes [`super::talent_version`], so the next
//! think of a segment regenerates its possible names.

use solstone_core_format::content::Family;

/// Routes the gate lets through (the evaluated `G2` configuration). A
/// disabled route still matches and still counts as a competitor; only its
/// actions are dropped, as evaluated.
pub(crate) const ROUTE_CANONICAL: bool = true;
pub(crate) const ROUTE_ALIAS: bool = true;
pub(crate) const ROUTE_FIRST: bool = false;
pub(crate) const ROUTE_PHONETIC: bool = false;
pub(crate) const ROUTE_SPELLING: bool = true;
/// An alias match is offered only when its entity is filed under one of the
/// segment's own facets (`talents/facets.json`); none listed means no match.
pub(crate) const ALIAS_NEEDS_SEGMENT_FACET: bool = true;

/// A name or alias is matched only when its compact key has this many
/// characters.
pub(crate) const MIN_FORM_CHARS: usize = 3;
pub(crate) const MAX_FORM_CHARS: usize = 64;
/// A form's sound key is indexed only from this length.
pub(crate) const MIN_SOUND_KEY_CHARS: usize = 3;
/// Spans are matched by sound or spelling only from this compact length.
pub(crate) const MIN_SOUND_SPAN_CHARS: usize = 3;
/// Spelling matches need forms and spans at least this long.
pub(crate) const MIN_SPELLING_CHARS: usize = 6;
/// The longest span tried, in words.
pub(crate) const MAX_SPAN_WORDS: usize = 6;
/// First names come from entities of this type with a name this many words
/// long, whose first word has this many compact characters.
pub(crate) const FIRST_NAME_TYPE: &str = "Person";
pub(crate) const MIN_FIRST_NAME_WORDS: usize = 2;
pub(crate) const MIN_FIRST_NAME_CHARS: usize = 3;
/// A sound match's span may be this much shorter or longer than the form.
pub(crate) const PHONETIC_MIN_LENGTH_RATIO: f64 = 0.67;
pub(crate) const PHONETIC_MAX_LENGTH_RATIO: f64 = 1.5;
/// A spelling match may differ in length by this many characters and must
/// reach this `rapidfuzz` ratio (0-100).
pub(crate) const SPELLING_MAX_LENGTH_DIFFERENCE: usize = 2;
pub(crate) const SPELLING_MIN_RATIO: f64 = 85.0;
/// A first name is offered only when at most this many other entities
/// matched the same words.
pub(crate) const FIRST_MAX_COMPETITORS: usize = 1;
/// The most candidates one request carries.
pub(crate) const MAX_CANDIDATES: usize = 12;
/// A candidate's note is cut to this many characters.
pub(crate) const NOTE_CHARS: usize = 200;
/// At most this many aliases are shown on a candidate.
pub(crate) const MAX_ALIASES_SHOWN: usize = 4;
/// Compact names of sol pbc's own products. The product spelling fix owns
/// them; they are never offered as possible names.
pub(crate) const PRODUCT_NAME_KEYS: [&str; 4] = ["sol", "solstone", "solpbc", "soulstone"];

/// Derived files whose indexed words already make a canonical name findable,
/// with the content family search renders each as: the activity, sense and
/// topics chunks the evaluated prototype read. `sense.json` is read as search
/// indexes it, and its rendering includes Sense's entity list.
pub(crate) const DERIVED_TEXT: [(&str, Family); 4] = [
    ("talents/activity.md", Family::Markdown),
    ("talents/sense.md", Family::Markdown),
    ("talents/sense.json", Family::Sense),
    ("talents/topics.md", Family::Markdown),
];
/// How [`DERIVED_TEXT`] is read: each file's index chunks, as the indexer renders
/// them (`solstone_core_format::content::produce_chunks`).
pub(crate) const DERIVED_TEXT_RENDERING: &str = "indexer produce_chunks";

/// Every constant above, in one stable text, for the talent version.
pub(crate) fn fingerprint() -> String {
    serde_json::json!({
        "routes": {
            "canonical": ROUTE_CANONICAL,
            "alias": ROUTE_ALIAS,
            "first": ROUTE_FIRST,
            "phonetic": ROUTE_PHONETIC,
            "spelling": ROUTE_SPELLING,
        },
        "alias_needs_segment_facet": ALIAS_NEEDS_SEGMENT_FACET,
        "form_chars": [MIN_FORM_CHARS, MAX_FORM_CHARS],
        "sound_key_chars": MIN_SOUND_KEY_CHARS,
        "sound_span_chars": MIN_SOUND_SPAN_CHARS,
        "spelling_chars": MIN_SPELLING_CHARS,
        "span_words": MAX_SPAN_WORDS,
        "first_name": [FIRST_NAME_TYPE, MIN_FIRST_NAME_WORDS.to_string(), MIN_FIRST_NAME_CHARS.to_string()],
        "phonetic_length_ratio": [PHONETIC_MIN_LENGTH_RATIO, PHONETIC_MAX_LENGTH_RATIO],
        "spelling": [SPELLING_MAX_LENGTH_DIFFERENCE as f64, SPELLING_MIN_RATIO],
        "first_max_competitors": FIRST_MAX_COMPETITORS,
        "candidates": MAX_CANDIDATES,
        "note_chars": NOTE_CHARS,
        "aliases_shown": MAX_ALIASES_SHOWN,
        "products": PRODUCT_NAME_KEYS,
        "derived_text": {
            "files": DERIVED_TEXT
                .iter()
                .map(|(file, family)| [(*file).to_owned(), format!("{family:?}")])
                .collect::<Vec<_>>(),
            "rendering": DERIVED_TEXT_RENDERING,
        },
    })
    .to_string()
}
