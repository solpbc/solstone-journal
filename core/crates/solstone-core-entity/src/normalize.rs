// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use caseless::default_case_fold_str;
use unicode_normalization::UnicodeNormalization;

/// Normalize a query for deterministic ambiguity identity.
pub fn normalize_resolution_query(query: &str) -> String {
    let normalized: String = query.nfkc().collect();
    let collapsed = collapse_python_whitespace(&normalized);
    // Python uses full case folding for ambiguity identity; unlike simple lowercase,
    // it handles equivalences such as ß → ss.
    default_case_fold_str(&collapsed)
}

pub(crate) fn matchable_resolution_query(query: &str) -> String {
    let normalized: String = query.nfkc().collect();
    collapse_python_whitespace(&normalized)
}

fn collapse_python_whitespace(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut pending_space = false;

    for ch in value.chars() {
        if is_python_regex_whitespace(ch) {
            pending_space = !output.is_empty();
        } else {
            if pending_space {
                output.push(' ');
                pending_space = false;
            }
            output.push(ch);
        }
    }

    output
}

// Python re.sub(r"\s+", ...) includes U+001C–U+001F, which char::is_whitespace()
// excludes. This exact table also covers Python's Unicode separators so identities do not drift.
fn is_python_regex_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'..='\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

#[cfg(test)]
mod tests {
    use super::{
        is_python_regex_whitespace, matchable_resolution_query, normalize_resolution_query,
    };

    #[test]
    fn python_regex_whitespace_table_is_exact() {
        for cp in 0u32..=0x10FFFF {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let expected = matches!(
                ch,
                '\u{09}'..='\u{0D}'
                    | '\u{1C}'..='\u{20}'
                    | '\u{85}'
                    | '\u{A0}'
                    | '\u{1680}'
                    | '\u{2000}'..='\u{200A}'
                    | '\u{2028}'..='\u{2029}'
                    | '\u{202F}'
                    | '\u{205F}'
                    | '\u{3000}'
            );
            assert_eq!(is_python_regex_whitespace(ch), expected, "U+{cp:04X}");
        }
    }

    #[test]
    fn python_regex_whitespace_collapses_c0_separators() {
        for ch in ['\u{1C}', '\u{1D}', '\u{1E}', '\u{1F}'] {
            assert!(is_python_regex_whitespace(ch));
        }
        assert_eq!(normalize_resolution_query("A\u{1C}B"), "a b");
    }

    #[test]
    fn matchable_resolution_query_normalizes_without_case_folding() {
        assert_eq!(matchable_resolution_query("  ﬁ  Straße  "), "fi Straße");
    }
}
