// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/fixtures.rs"]
mod fixtures;

use regex::Regex;
use solstone_core_system_health::{GrepCompileError, compile_grep_pattern, decimal_digit_value};

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
    Disposition::Unsupported("numeric-escape", 5),
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
        assert_eq!(
            decimal_digit_value(scalar.chars().next().unwrap()),
            Some(value),
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
    for pattern in ["[a--b]", "[a~~b]", "[a||b]"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "ambiguous-character-class",
                offset: 2
            })
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
    for pattern in [
        "\\n",
        "\\t",
        "\\x41",
        "\\u0041",
        "\\uD800",
        "\\U0000D800",
        "\\U0001F642",
        "\\N{BULLET}",
        "\\N{BACKSPACE}",
        "\\N{HANGUL SYLLABLE GA}",
        "\\N{CJK UNIFIED IDEOGRAPH-4E00}",
        "\\N{bullet}",
    ] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "character-escape",
                offset: 0
            })
        ));
    }
    for pattern in [
        "\\x4",
        "\\c",
        "\\U00110000",
        "\\N{}",
        "\\N{NO SUCH UNICODE NAME}",
        "\\N{CJK  UNIFIED IDEOGRAPH-4E00}",
    ] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: 0 })
        ));
    }
    assert!(compile_grep_pattern("\\.").unwrap().is_match("."));
    assert!(compile_grep_pattern("\\é").unwrap().is_match("é"));
}

#[test]
fn every_closed_group_and_numeric_family_has_exact_disposition() {
    for pattern in [
        "(?m)foo",
        "(?ii)foo",
        "(?s:foo)",
        "(?-i:foo)",
        "(?-ii:foo)",
        "(?im-sx:foo)",
    ] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "inline-flags",
                offset: 0
            })
        ));
    }
    for pattern in [
        "(?-u:foo)",
        "(?a-u:foo)",
        "(?au:foo)",
        "(?i-i:foo)",
        "(?q)foo",
    ] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: 0 })
        ));
    }
    for pattern in ["a(?i)", "(?:(?i))"] {
        let offset = pattern.find("(?i)").unwrap();
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: actual }) if actual == offset
        ));
    }
    assert!(matches!(
        compile_grep_pattern("a(?i:b)"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "inline-flags",
            offset: 1
        })
    ));
    for (pattern, family) in [
        ("(a)?(?(1)yes|no)", "conditional"),
        ("(?#note)", "comment"),
        ("(?>a)", "atomic-group"),
        ("(?P<n>a)", "named-group"),
        ("(?P<a\u{301}>a)", "named-group"),
        ("(?P<℘>a)", "named-group"),
        ("(?P<a·b>a)", "named-group"),
        ("(?P<n>a)(?P=n)", "named-group"),
        ("a(?=b)", "lookaround"),
    ] {
        let expected_offset = pattern.find("(?").unwrap();
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily { family: actual, offset })
                if actual == family && offset == expected_offset
        ));
    }
    // This closed subset recognizes named-backreference syntax lexically and
    // rejects the entire family before resolving whether the name was defined.
    assert!(matches!(
        compile_grep_pattern("(?P=n)"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "backreference",
            offset: 0
        })
    ));
    assert!(matches!(
        compile_grep_pattern("(?#"),
        Err(GrepCompileError::InvalidPattern { offset: 0 })
    ));
    assert!(matches!(
        compile_grep_pattern("(?(1)yes|no)"),
        Err(GrepCompileError::InvalidPattern { offset: 0 })
    ));
    for pattern in ["(?P<", "(?P<n>", "(?P=", "(?=", "(?<=", "(?>"] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: 0 })
        ));
    }
    for pattern in [
        "(?P<1a>a)",
        "(?P<a-b>a)",
        "(?P<a\u{200c}>a)",
        "(?P<a\u{200d}>a)",
    ] {
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::InvalidPattern { offset: 0 })
        ));
    }
    for pattern in ["\\0", "\\1", "x\\9"] {
        let offset = pattern.find('\\').unwrap();
        assert!(matches!(
            compile_grep_pattern(pattern),
            Err(GrepCompileError::UnsupportedFamily {
                family: "numeric-escape",
                offset: actual
            }) if actual == offset
        ));
    }
}

#[test]
fn ranges_and_quantifiers_are_validated_before_native_compile() {
    for pattern in [
        "[a-c]", "[-a]", "[a-]", "[a-b-a]", "[a-b-c]", "a{2}", "a{2,}", "a{2,3}", "a{2,3}?",
    ] {
        compile_grep_pattern(pattern).unwrap_or_else(|error| panic!("{pattern:?}: {error:?}"));
    }
    for pattern in ["[a-b-a]", "[a-b-c]"] {
        let compiled = compile_grep_pattern(pattern).unwrap();
        for admitted in ['a', 'b', '-', 'c'] {
            if admitted == 'c' && pattern == "[a-b-a]" {
                assert!(!compiled.is_match("c"));
            } else {
                assert!(
                    compiled.is_match(&admitted.to_string()),
                    "{pattern:?} {admitted:?}"
                );
            }
        }
        assert!(!compiled.is_match("z"));
    }
    for (pattern, offset) in [
        ("[z-a]", 2),
        ("[\\d-a]", 3),
        ("*a", 0),
        ("a**", 2),
        ("a{3,2}", 1),
        ("a{4294967296}", 1),
        ("a{2", 1),
    ] {
        assert!(
            matches!(
                compile_grep_pattern(pattern),
                Err(GrepCompileError::InvalidPattern { offset: actual }) if actual == offset
            ),
            "{pattern:?}"
        );
    }
}

#[test]
fn earliest_decisive_token_and_original_utf8_offsets_win() {
    assert!(matches!(
        compile_grep_pattern("(?i)\\c"),
        Err(GrepCompileError::UnsupportedFamily {
            family: "inline-flags",
            offset: 0
        })
    ));
    assert!(matches!(
        compile_grep_pattern("\\c(?i)"),
        Err(GrepCompileError::InvalidPattern { offset: 0 })
    ));
    assert!(matches!(
        compile_grep_pattern("é\\1["),
        Err(GrepCompileError::UnsupportedFamily {
            family: "numeric-escape",
            offset: 2
        })
    ));
    assert!(matches!(
        compile_grep_pattern("é\\c(?i)"),
        Err(GrepCompileError::InvalidPattern { offset: 2 })
    ));
    assert!(matches!(
        compile_grep_pattern("é(a"),
        Err(GrepCompileError::InvalidPattern { offset: 2 })
    ));
    assert!(matches!(
        compile_grep_pattern("é[a"),
        Err(GrepCompileError::InvalidPattern { offset: 2 })
    ));
}
