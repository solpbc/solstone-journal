// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/fixtures.rs"]
mod fixtures;

use regex::Regex;
use solstone_core_system_health::{GrepCompileError, compile_grep_pattern};

enum Disposition {
    Admit,
    Unsupported(&'static str, usize),
    Invalid(usize),
}

// This is AC2's closed-subset table.  It is intentionally independent of the
// fixture's unrestricted Python `re.compile` Outcome/Error variant.
const DISPOSITIONS: [Disposition; 36] = [
    Disposition::Admit,
    Disposition::Admit,
    Disposition::Admit,
    Disposition::Unsupported("character-class-semantics", 0),
    Disposition::Admit,
    Disposition::Unsupported("character-class-semantics", 0),
    Disposition::Unsupported("character-class-semantics", 0),
    Disposition::Unsupported("inline-flags", 0),
    Disposition::Unsupported("inline-flags", 0),
    Disposition::Unsupported("inline-flags", 0),
    Disposition::Unsupported("inline-flags", 0),
    Disposition::Invalid(0),
    Disposition::Invalid(0),
    Disposition::Invalid(0),
    Disposition::Invalid(0),
    Disposition::Invalid(0),
    Disposition::Unsupported("ambiguous-character-class", 2),
    Disposition::Admit,
    Disposition::Unsupported("inline-flags", 0),
    Disposition::Admit,
    Disposition::Admit,
    Disposition::Unsupported("named-group", 0),
    Disposition::Unsupported("lookaround", 3),
    Disposition::Unsupported("lookaround", 3),
    Disposition::Unsupported("lookaround", 0),
    Disposition::Unsupported("lookaround", 0),
    Disposition::Unsupported("backreference", 5),
    Disposition::Unsupported("named-group", 0),
    Disposition::Unsupported("atomic-group", 0),
    Disposition::Unsupported("possessive-quantifier", 3),
    Disposition::Unsupported("named-group", 0),
    Disposition::Invalid(0),
    Disposition::Unsupported("ambiguous-character-class", 1),
    Disposition::Admit,
    Disposition::Invalid(0),
    Disposition::Invalid(0),
];

#[test]
fn every_pinned_regex_case_has_its_closed_subset_disposition() {
    fixtures::assert_fixture_shapes();
    let fixture = fixtures::health_logs_fixture();
    assert_eq!(fixture.regex.len(), DISPOSITIONS.len());
    for (index, (case, disposition)) in fixture.regex.iter().zip(DISPOSITIONS.iter()).enumerate() {
        let (pattern, haystacks, matches) = match case {
            fixtures::RegexCase::Outcome(case) => {
                (&case.pattern, &case.haystacks, Some(&case.matches))
            }
            fixtures::RegexCase::Error(case) => (&case.pattern, &case.haystacks, None),
        };
        match disposition {
            Disposition::Admit => {
                let compiled = compile_grep_pattern(pattern)
                    .unwrap_or_else(|error| panic!("case {index} {pattern:?}: {error:?}"));
                let expected = matches.expect("admitted pattern has Python outcome");
                assert_eq!(haystacks.len(), expected.len());
                for (haystack, expected) in haystacks.iter().zip(expected) {
                    assert_eq!(
                        compiled.is_match(haystack),
                        *expected,
                        "case {index} {pattern:?} on {haystack:?}"
                    );
                }
            }
            Disposition::Unsupported(family, offset) => assert!(
                matches!(compile_grep_pattern(pattern), Err(GrepCompileError::UnsupportedFamily { family: actual_family, offset: actual_offset }) if actual_family == *family && actual_offset == *offset),
                "case {index} {pattern:?}"
            ),
            Disposition::Invalid(offset) => assert!(
                matches!(compile_grep_pattern(pattern), Err(GrepCompileError::InvalidPattern { offset: actual_offset }) if actual_offset == *offset),
                "case {index} {pattern:?}"
            ),
        }
    }
}

#[test]
fn decimal_translation_matches_all_unicode_16_decimal_scalars() {
    let text = fixtures::health_text_fixture();
    let digits = compile_grep_pattern("\\d").unwrap();
    let non_digits = compile_grep_pattern("\\D").unwrap();
    for &(scalar, value, _, _) in &text.decimal_cases {
        let scalar = char::from_u32(scalar).unwrap().to_string();
        assert!(digits.is_match(&scalar), "decimal {scalar:?} value {value}");
        assert!(
            !non_digits.is_match(&scalar),
            "decimal {scalar:?} value {value}"
        );
    }
    let inside_class = compile_grep_pattern("[\\dA-Z]").unwrap();
    assert!(inside_class.is_match("٣"));
    assert!(inside_class.is_match("Q"));
    assert!(!inside_class.is_match("é"));
}

#[test]
fn ascii_only_digit_shortcut_diverges_from_the_fixture() {
    let wrong = Regex::new("[0-9]").unwrap();
    let text = fixtures::health_text_fixture();
    assert!(text.decimal_cases.iter().any(|(scalar, _, _, _)| {
        let scalar = char::from_u32(*scalar).unwrap().to_string();
        !wrong.is_match(&scalar)
    }));
}

#[test]
fn brace_and_class_lookalike_twins_are_closed_before_native_compile() {
    assert!(matches!(
        compile_grep_pattern("a{b}"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "brace-spelling",
            offset: 1
        })
    ));
    for pattern in ["a{}", "a{,}", "a{,3}"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "brace-spelling",
                offset: 1
            })
        ));
    }
    for pattern in ["{2}", "a{2"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { .. })
        ));
    }
    assert!(matches!(
        compile_grep_pattern("[a&&b]"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "ambiguous-character-class",
            offset: 2
        })
    ));
    assert!(matches!(
        compile_grep_pattern("[a&&b]"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "ambiguous-character-class",
            offset: 2
        })
    ));
    assert!(matches!(
        compile_grep_pattern("[[:alpha:]]"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "ambiguous-character-class",
            offset: 1
        })
    ));
}

#[test]
fn end_anchors_are_only_admitted_once_at_the_literal_pattern_end() {
    for pattern in ["foo$|bar", "(?:foo$)", "foo$$"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "nonterminal-dollar-anchor",
                ..
            })
        ));
    }
    for pattern in ["foo\\Z|bar$", "foo\\z|bar$"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "mixed-end-anchors",
                ..
            })
        ));
    }
    assert!(compile_grep_pattern("^foo$").unwrap().is_match("foo"));
    assert!(compile_grep_pattern("foo$").unwrap().is_match("foo\n"));
}

#[test]
fn character_escape_family_is_closed_without_closing_literal_escapes() {
    for pattern in ["\\n", "\\t", "\\x41"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "character-escape",
                offset: 0
            })
        ));
    }
    for pattern in ["\\x4", "\\c", "\\U00110000", "\\U0000D800", "\\N{}"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: 0 })
        ));
    }
    assert!(matches!(
        compile_grep_pattern("\\N{BULLET}"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "character-escape",
            offset: 0
        })
    ));
    assert!(compile_grep_pattern("\\.").unwrap().is_match("."));
    assert!(compile_grep_pattern("\\é").unwrap().is_match("é"));
}
