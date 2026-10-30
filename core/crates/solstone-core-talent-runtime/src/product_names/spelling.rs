// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub view_start: usize,
    pub view_end: usize,
    pub original: String,
    pub replacement: String,
    /// Set when the replacement is a possible name: the entity it names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

/// A possible name read in place of the words it labels, by byte range.
pub struct Named<'a> {
    pub start: usize,
    pub end: usize,
    pub name: &'a str,
    pub entity_id: &'a str,
}

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)soulstone|soul[ \t]+pbc|soul[ \t]+pb[ \t]+c|sol[ \t]+p[ \t]+bc")
        .expect("fixed product spellings")
});
// L/N plus underscore is Python's Unicode word class; Rust regex's \w also
// includes combining marks and other connector punctuation.
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").unwrap());
static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)`[^`]*`|(?:https?://|www\.)\S+|(?:[/\\][\p{L}\p{N}_.~-]+)+(?:[/\\][\p{L}\p{N}_.~-]+)*|[\p{L}\p{N}_.+-]+@[\p{L}\p{N}_.-]+|[\p{L}\p{N}_-]+(?:\.[\p{L}\p{N}_-]+)+"
).expect("fixed literal boundaries")
});

fn word(c: char) -> bool {
    WORD.is_match(c.encode_utf8(&mut [0; 4]))
}

pub fn has_variant(text: &str) -> bool {
    NAME.is_match(text)
}

pub fn normalize(text: &str) -> (String, Vec<Edit>) {
    normalize_with(text, &[])
}

/// [`normalize`], plus each possible name read in place of its words. One
/// whose words overlap a product spelling or another possible name, or don't
/// sit on character boundaries, is left out.
pub fn normalize_with(text: &str, named: &[Named<'_>]) -> (String, Vec<Edit>) {
    let mut planned = product_matches(text);
    for n in named {
        if n.start >= n.end
            || !text.is_char_boundary(n.start)
            || !text.is_char_boundary(n.end)
            || planned.iter().any(|(s, e, ..)| n.start < *e && n.end > *s)
        {
            continue;
        }
        planned.push((n.start, n.end, n.name, Some(n.entity_id)));
    }
    planned.sort_by_key(|p| p.0);
    let mut out = String::with_capacity(text.len());
    let mut edits = Vec::new();
    let mut cursor = 0;
    for (start, end, replacement, entity_id) in planned {
        out.push_str(&text[cursor..start]);
        let view_start = out.len();
        out.push_str(replacement);
        edits.push(Edit {
            start,
            end,
            view_start,
            view_end: out.len(),
            original: text[start..end].to_owned(),
            replacement: replacement.to_owned(),
            entity_id: entity_id.map(str::to_owned),
        });
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    (out, edits)
}

fn product_matches<'a>(text: &str) -> Vec<(usize, usize, &'a str, Option<&'a str>)> {
    let protected: Vec<_> = LITERAL.find_iter(text).map(|m| m.range()).collect();
    let mut matches = Vec::new();
    for m in NAME.find_iter(text) {
        if text[..m.start()].chars().next_back().is_some_and(word)
            || text[m.end()..].chars().next().is_some_and(word)
            || protected
                .iter()
                .any(|p| m.start() < p.end && m.end() > p.start)
        {
            continue;
        }
        let replacement = if m.as_str().eq_ignore_ascii_case("soulstone")
            || m.as_str().to_lowercase().replace('ſ', "s") == "soulstone"
        {
            "solstone"
        } else {
            "sol pbc"
        };
        matches.push((m.start(), m.end(), replacement, None));
    }
    matches
}

/// A normalized substring's exact original byte range. Partial name edits
/// cannot supply a source span.
pub fn original_range(start: usize, end: usize, edits: &[Edit]) -> Option<std::ops::Range<usize>> {
    let boundary = |at: usize| {
        let mut delta = 0isize;
        for e in edits {
            if at > e.view_start && at < e.view_end {
                return None;
            }
            if at >= e.view_end {
                delta += (e.end - e.start) as isize - (e.view_end - e.view_start) as isize;
            }
        }
        at.checked_add_signed(delta)
    };
    Some(boundary(start)?..boundary(end)?)
}

pub fn quote_ranges(raw: &str, quote: &str) -> Vec<std::ops::Range<usize>> {
    if quote.is_empty() {
        return Vec::new();
    }
    let (view, edits) = normalize(raw);
    view.match_indices(quote)
        .filter_map(|(at, _)| original_range(at, at + quote.len(), &edits))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_literals_and_reversible_unicode_spans() {
        let raw = "journal éSoulstone Soulstones SoulstoneApp Soulstoneé e\u{301}Soulstone — Soul PB C / sol P BC";
        let (view, edits) = normalize(raw);
        assert_eq!(
            view,
            "journal éSoulstone Soulstones SoulstoneApp Soulstoneé e\u{301}solstone — sol pbc / sol pbc"
        );
        for e in edits {
            assert_eq!(&raw[e.start..e.end], e.original);
            assert_eq!(&view[e.view_start..e.view_end], e.replacement);
        }
        for literal in [
            "Soulstone.csv",
            "Soulstone.cpp",
            "info@soulstone.example",
            "/tmp/Soulstone",
            "`Soulstone`",
            "https://example.org/Soulstone",
        ] {
            assert_eq!(normalize(literal).0, literal);
            assert!(normalize(literal).1.is_empty());
        }
        assert_eq!(
            normalize("soul stone; sol stone").0,
            "soul stone; sol stone"
        );
    }
    #[test]
    fn quotes_map_only_at_complete_edit_boundaries() {
        let raw = "We tested Soulstone journal on Friday.";
        let ranges = quote_ranges(raw, "tested solstone journal on Friday");
        assert_eq!(
            &raw[ranges[0].clone()],
            "tested Soulstone journal on Friday"
        );
        assert!(quote_ranges(raw, "solsto").is_empty());
    }
}
