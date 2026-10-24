// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A small English sound key: two spellings that sound alike usually share
//! one ("Fryer" and "Friar" are both `FR`). It only retrieves candidates; the
//! model decides. Ported line for line from the evaluated prototype, so its
//! keys are the tuned ones.

use unicode_normalization::UnicodeNormalization;

/// Digraph rewrites, applied in order to the whole folded word.
const RULES: [(&str, &str); 13] = [
    ("tch", "x"),
    ("sch", "sk"),
    ("tion", "xn"),
    ("sion", "xn"),
    ("ph", "f"),
    ("ck", "k"),
    ("sh", "x"),
    ("ch", "x"),
    ("th", "0"),
    ("wh", "w"),
    ("qu", "kw"),
    ("dg", "j"),
    ("gh", "g"),
];
/// Silent or softened word starts; only the first that matches applies.
const INITIAL: [(&str, &str); 6] = [
    ("kn", "n"),
    ("gn", "n"),
    ("pn", "n"),
    ("wr", "r"),
    ("ps", "s"),
    ("x", "s"),
];

/// Compatibility-decomposed, lowercased, ASCII letters only.
fn fold(text: &str) -> String {
    text.nfkd()
        .flat_map(char::to_lowercase)
        .filter(char::is_ascii_lowercase)
        .collect()
}

/// The sound key of `text`; empty when it has no ASCII letters.
pub(crate) fn key(text: &str) -> String {
    let mut word = fold(text);
    if word.is_empty() {
        return String::new();
    }
    for (from, to) in INITIAL {
        if let Some(rest) = word.strip_prefix(from) {
            word = format!("{to}{rest}");
            break;
        }
    }
    for (from, to) in RULES {
        word = word.replace(from, to);
    }
    let letters = word.chars().collect::<Vec<_>>();
    let mut out = Vec::<char>::new();
    for (index, &letter) in letters.iter().enumerate() {
        let next = letters.get(index + 1).copied();
        let soft = matches!(next, Some('e' | 'i' | 'y'));
        let letter = match letter {
            'c' if soft => 's',
            'c' => 'k',
            'g' if soft => 'j',
            'q' => 'k',
            'z' => 's',
            'v' => 'f',
            other => other,
        };
        let kept = match letter {
            'a' | 'e' | 'i' | 'o' | 'u' => (index == 0).then_some('a'),
            'h' => None,
            'w' | 'y' => (index == 0).then_some(letter),
            other => Some(other),
        };
        if let Some(kept) = kept
            && out.last() != Some(&kept)
        {
            out.push(kept);
        }
    }
    out.into_iter().collect::<String>().to_uppercase()
}
