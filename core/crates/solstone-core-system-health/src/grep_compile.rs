// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A deliberately small Python-`re` grep subset.

use regex::Regex;

/// A compile error classified before the native regex engine sees closed syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrepCompileError {
    UnsupportedFamily { family: &'static str, offset: usize },
    InvalidPattern { offset: usize },
    NativeCompileFailure,
}

/// A compiled grep pattern with Python's final-newline `$` search behavior.
#[derive(Debug, Clone)]
pub struct GrepPattern {
    regex: Regex,
    retry_without_final_lf: bool,
}

impl GrepPattern {
    /// Search a complete raw log line.
    #[must_use]
    pub fn is_match(&self, haystack: &str) -> bool {
        self.regex.is_match(haystack)
            || (self.retry_without_final_lf
                && haystack
                    .strip_suffix('\n')
                    .is_some_and(|line| self.regex.is_match(line)))
    }
}

// Unicode 16.0.0's 76 decimal-digit blocks, represented by each block's zero.
const DECIMAL_ZEROS: &[u32; 76] = &[
    0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66, 0x0ce6,
    0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
    0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
    0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
    0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
    0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
];

/// Return the Unicode 16.0.0 decimal value for `scalar`, if it is an `Nd`
/// character.  The grep compiler and callers that must match Python's
/// decimal-digit semantics share the same pinned block table.
#[must_use]
pub fn decimal_digit_value(scalar: char) -> Option<u8> {
    let scalar = scalar as u32;
    DECIMAL_ZEROS
        .binary_search_by(|zero| {
            if scalar < *zero {
                std::cmp::Ordering::Greater
            } else if scalar > *zero + 9 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .ok()
        .map(|index| (scalar - DECIMAL_ZEROS[index]) as u8)
}

fn decimal_members() -> String {
    let mut output = String::new();
    for zero in DECIMAL_ZEROS {
        use std::fmt::Write as _;
        let _ = write!(output, "\\x{{{zero:x}}}-\\x{{{:x}}}", zero + 9);
    }
    output
}

/// Compile the closed native grep subset. Offsets are UTF-8 byte offsets in
/// the original user pattern.
pub fn compile_grep_pattern(pattern: &str) -> Result<GrepPattern, GrepCompileError> {
    compile_grep_pattern_with(pattern, &mut |translated| Regex::new(translated))
}

fn compile_grep_pattern_with<F>(
    pattern: &str,
    builder: &mut F,
) -> Result<GrepPattern, GrepCompileError>
where
    F: FnMut(&str) -> Result<Regex, regex::Error>,
{
    let mut output = String::with_capacity(pattern.len());
    let mut cursor = 0;
    let mut group_openings = Vec::new();
    let mut retry_without_final_lf = false;
    let mut seen_dollar = false;
    let mut seen_exact_end_anchor = false;
    let mut can_quantify = false;
    let mut can_be_lazy = false;
    let mut capture_count = 0_usize;
    while cursor < pattern.len() {
        let (scalar, next) = next_scalar(pattern, cursor)?;
        match scalar {
            '\\' => {
                let (escaped, after) = next_scalar(pattern, next)?;
                match escaped {
                    'd' => {
                        output.push_str(&format!("[{}]", decimal_members()));
                        can_quantify = true;
                    }
                    'D' => {
                        output.push_str(&format!("[^{}]", decimal_members()));
                        can_quantify = true;
                    }
                    'A' => {
                        output.push_str("\\A");
                        can_quantify = false;
                    }
                    'Z' | 'z' => {
                        output.push_str("\\z");
                        seen_exact_end_anchor = true;
                        can_quantify = false;
                    }
                    '0'..='9' => return unsupported("numeric-escape", cursor),
                    'b' | 'B' | 's' | 'S' | 'w' | 'W' => {
                        return unsupported("character-class-semantics", cursor);
                    }
                    'p' | 'P' | 'R' => return invalid(cursor),
                    'a' | 'f' | 'n' | 'r' | 't' | 'v' => {
                        return unsupported("character-escape", cursor);
                    }
                    'x' | 'u' | 'U' | 'N' => {
                        validate_character_escape(pattern, cursor, escaped, after)?;
                        return unsupported("character-escape", cursor);
                    }
                    _ if escaped.is_ascii_alphabetic() => return invalid(cursor),
                    _ if !escaped.is_ascii() => {
                        output.push(escaped);
                        can_quantify = true;
                    }
                    _ => {
                        output.push('\\');
                        output.push(escaped);
                        can_quantify = true;
                    }
                }
                cursor = after;
                can_be_lazy = false;
            }
            '[' => {
                let (class, after) = translate_class(pattern, cursor)?;
                output.push_str(&class);
                cursor = after;
                can_quantify = true;
                can_be_lazy = false;
            }
            ']' => return invalid(cursor),
            '(' => {
                if pattern[next..].starts_with('?') {
                    let offset = cursor;
                    let rest = &pattern[next + 1..];
                    if rest.starts_with("P<") {
                        return if named_group_is_well_formed(pattern, offset, rest) {
                            unsupported("named-group", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if rest.starts_with("P=") {
                        return if named_backreference_is_well_formed(rest) {
                            unsupported("backreference", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if rest.starts_with('=')
                        || rest.starts_with('!')
                        || rest.starts_with("<=")
                        || rest.starts_with("<!")
                    {
                        return if group_has_closing(pattern, offset) {
                            unsupported("lookaround", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if rest.starts_with('>') {
                        return if group_has_closing(pattern, offset) {
                            unsupported("atomic-group", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if rest.starts_with('(') {
                        return if conditional_group_is_well_formed(rest, capture_count) {
                            unsupported("conditional", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if let Some(comment) = rest.strip_prefix('#') {
                        return if comment.contains(')') {
                            unsupported("comment", offset)
                        } else {
                            invalid(offset)
                        };
                    }
                    if let Some(flags) = inline_flags_kind(rest) {
                        return match flags {
                            InlineFlags::Global if offset != 0 => invalid(offset),
                            InlineFlags::Global | InlineFlags::Scoped => {
                                unsupported("inline-flags", offset)
                            }
                        };
                    }
                    if rest.starts_with('R')
                        || rest.starts_with('U')
                        || rest.starts_with('L')
                        || rest.starts_with('<')
                    {
                        return invalid(offset);
                    }
                    if rest.starts_with(':') {
                        output.push_str("(?:");
                        cursor = next + 2;
                        group_openings.push(offset);
                        can_quantify = false;
                        can_be_lazy = false;
                        continue;
                    }
                    return invalid(offset);
                }
                output.push('(');
                capture_count += 1;
                group_openings.push(cursor);
                cursor = next;
                can_quantify = false;
                can_be_lazy = false;
            }
            ')' => {
                if group_openings.pop().is_none() {
                    return invalid(cursor);
                }
                output.push(')');
                cursor = next;
                can_quantify = true;
                can_be_lazy = false;
            }
            '$' => {
                if next != pattern.len() || seen_dollar {
                    return unsupported("nonterminal-dollar-anchor", cursor);
                }
                if seen_exact_end_anchor {
                    return unsupported("mixed-end-anchors", cursor);
                }
                output.push_str("\\z");
                retry_without_final_lf = true;
                seen_dollar = true;
                cursor = next;
                can_quantify = false;
                can_be_lazy = false;
            }
            '{' => {
                if !can_quantify {
                    return invalid(cursor);
                }
                let (quantifier, after) = parse_quantifier(pattern, cursor)?;
                output.push_str(&quantifier);
                cursor = after;
                can_quantify = false;
                can_be_lazy = !quantifier.ends_with('?');
            }
            '+' | '*' | '?' => {
                if scalar == '?' && can_be_lazy {
                    output.push('?');
                    cursor = next;
                    can_be_lazy = false;
                    can_quantify = false;
                    continue;
                }
                if !can_quantify {
                    return invalid(cursor);
                }
                if pattern[next..].starts_with('+') {
                    return unsupported("possessive-quantifier", cursor);
                }
                output.push(scalar);
                cursor = next;
                can_quantify = false;
                can_be_lazy = true;
            }
            _ => {
                output.push(scalar);
                cursor = next;
                can_quantify = !matches!(scalar, '^' | '|');
                can_be_lazy = false;
            }
        }
    }
    if let Some(opening) = group_openings.last() {
        return invalid(*opening);
    }
    let regex = builder(&output).map_err(|_| GrepCompileError::NativeCompileFailure)?;
    Ok(GrepPattern {
        regex,
        retry_without_final_lf,
    })
}

#[derive(Clone, Copy)]
enum InlineFlags {
    Global,
    Scoped,
}

fn inline_flags_kind(rest: &str) -> Option<InlineFlags> {
    let bytes = rest.as_bytes();
    let mut cursor = 0;
    let mut positive = [false; 6];
    let mut positive_count = 0;
    while let Some(&byte) = bytes.get(cursor) {
        let Some(index) = flag_index(byte) else { break };
        positive[index] = true;
        positive_count += 1;
        cursor += 1;
    }
    if positive[0] && positive[4] {
        return None;
    }
    if bytes.get(cursor) == Some(&b')') {
        return (positive_count != 0).then_some(InlineFlags::Global);
    }
    let mut removed_count = 0;
    if bytes.get(cursor) == Some(&b'-') {
        cursor += 1;
        while let Some(&byte) = bytes.get(cursor) {
            if removable_flag_index(byte).is_none() {
                break;
            }
            if positive[flag_index(byte).expect("removable flag is known")] {
                return None;
            }
            removed_count += 1;
            cursor += 1;
        }
    }
    (bytes.get(cursor) == Some(&b':') && positive_count + removed_count != 0)
        .then_some(InlineFlags::Scoped)
}

fn named_group_is_well_formed(pattern: &str, opening: usize, rest: &str) -> bool {
    let Some(name_end) = rest[2..].find('>') else {
        return false;
    };
    let name = &rest[2..2 + name_end];
    valid_group_name(name) && group_has_closing(pattern, opening)
}

fn named_backreference_is_well_formed(rest: &str) -> bool {
    let Some(name_end) = rest[2..].find(')') else {
        return false;
    };
    valid_group_name(&rest[2..2 + name_end])
}

fn valid_group_name(name: &str) -> bool {
    use unicode_xid::UnicodeXID as _;

    let mut scalars = name.chars();
    scalars
        .next()
        .is_some_and(|scalar| scalar == '_' || scalar.is_xid_start())
        && scalars
            .all(|scalar| !matches!(scalar, '\u{200c}' | '\u{200d}') && scalar.is_xid_continue())
}

fn group_has_closing(pattern: &str, opening: usize) -> bool {
    let mut depth = 0_usize;
    let mut escaped = false;
    let mut in_class = false;
    for scalar in pattern[opening..].chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if scalar == '\\' {
            escaped = true;
            continue;
        }
        if in_class {
            if scalar == ']' {
                in_class = false;
            }
            continue;
        }
        match scalar {
            '[' => in_class = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn conditional_group_is_well_formed(rest: &str, capture_count: usize) -> bool {
    let Some(condition_end) = rest[1..].find(')') else {
        return false;
    };
    let condition = &rest[1..1 + condition_end];
    condition
        .parse::<usize>()
        .ok()
        .is_some_and(|group| group != 0 && group <= capture_count)
        && rest[2 + condition_end..].contains(')')
}

fn flag_index(flag: u8) -> Option<usize> {
    match flag {
        b'a' => Some(0),
        b'i' => Some(1),
        b'm' => Some(2),
        b's' => Some(3),
        b'u' => Some(4),
        b'x' => Some(5),
        _ => None,
    }
}

fn removable_flag_index(flag: u8) -> Option<usize> {
    match flag {
        b'i' => Some(0),
        b'm' => Some(1),
        b's' => Some(2),
        b'x' => Some(3),
        _ => None,
    }
}

fn translate_class(pattern: &str, start: usize) -> Result<(String, usize), GrepCompileError> {
    let mut cursor = start + 1;
    let mut negated = false;
    if pattern[cursor..].starts_with('^') {
        negated = true;
        cursor += 1;
    }
    let mut positive = String::new();
    let mut complement_decimal = false;
    let mut atom_count = 0_usize;
    let mut last_literal = None;
    let mut pending_range = None;
    let mut range_just_completed = false;
    while cursor < pattern.len() {
        let (scalar, next) = next_scalar(pattern, cursor)?;
        match scalar {
            ']' => {
                if atom_count == 0 {
                    positive.push_str("\\]");
                    atom_count += 1;
                    last_literal = Some(']');
                    cursor = next;
                    continue;
                }
                if pending_range.is_some() {
                    positive.push_str("\\-");
                }
                let decimals = decimal_members();
                let rendered = if complement_decimal {
                    if negated {
                        if positive.is_empty() {
                            format!("[{decimals}]")
                        } else {
                            format!("[{decimals}&&[^{positive}]]")
                        }
                    } else if positive.is_empty() {
                        format!("[^{decimals}]")
                    } else {
                        format!("(?:[^{decimals}]|[{positive}])")
                    }
                } else if negated {
                    format!("[^{positive}]")
                } else {
                    format!("[{positive}]")
                };
                return Ok((rendered, next));
            }
            '[' => return unsupported("ambiguous-character-class", cursor),
            '&' | '-' | '~' | '|'
                if pattern[next..]
                    .chars()
                    .next()
                    .is_some_and(|following| following == scalar) =>
            {
                return unsupported("ambiguous-character-class", cursor);
            }
            '-' if atom_count == 0 => {
                positive.push_str("\\-");
                atom_count += 1;
                last_literal = None;
                range_just_completed = false;
                cursor = next;
            }
            '-' if pattern[next..].starts_with(']') => {
                positive.push_str("\\-");
                atom_count += 1;
                last_literal = None;
                range_just_completed = false;
                cursor = next;
            }
            '-' if range_just_completed => {
                positive.push_str("\\-");
                atom_count += 1;
                last_literal = None;
                range_just_completed = false;
                cursor = next;
            }
            '-' => {
                let Some(first) = last_literal else {
                    return invalid(cursor);
                };
                positive.push('-');
                pending_range = Some((first, cursor));
                last_literal = None;
                cursor = next;
            }
            '\\' => {
                let (escaped, after) = next_scalar(pattern, next)?;
                match escaped {
                    'd' => {
                        if pending_range.is_some() {
                            return invalid(cursor);
                        }
                        positive.push_str(&decimal_members());
                        last_literal = None;
                        range_just_completed = false;
                    }
                    'D' => {
                        if pending_range.is_some() {
                            return invalid(cursor);
                        }
                        complement_decimal = true;
                        last_literal = None;
                        range_just_completed = false;
                    }
                    'b' | 'B' | 's' | 'S' | 'w' | 'W' => {
                        return unsupported("character-class-semantics", cursor);
                    }
                    'p' | 'P' | 'R' | 'A' | 'Z' | 'z' => return invalid(cursor),
                    '0'..='9' => return unsupported("numeric-escape", cursor),
                    'a' | 'f' | 'n' | 'r' | 't' | 'v' => {
                        return unsupported("character-escape", cursor);
                    }
                    'x' | 'u' | 'U' | 'N' => {
                        validate_character_escape(pattern, cursor, escaped, after)?;
                        return unsupported("character-escape", cursor);
                    }
                    _ if escaped.is_ascii_alphabetic() => return invalid(cursor),
                    _ if !escaped.is_ascii() => {
                        range_just_completed = validate_range_end(&mut pending_range, escaped)?;
                        positive.push(escaped);
                        last_literal = Some(escaped);
                    }
                    _ => {
                        range_just_completed = validate_range_end(&mut pending_range, escaped)?;
                        positive.push('\\');
                        positive.push(escaped);
                        last_literal = Some(escaped);
                    }
                }
                atom_count += 1;
                cursor = after;
            }
            _ => {
                range_just_completed = validate_range_end(&mut pending_range, scalar)?;
                positive.push(scalar);
                atom_count += 1;
                last_literal = Some(scalar);
                cursor = next;
            }
        }
    }
    invalid(start)
}

fn validate_range_end(
    pending: &mut Option<(char, usize)>,
    end: char,
) -> Result<bool, GrepCompileError> {
    let completed = pending.is_some();
    if let Some((start, offset)) = pending.take()
        && start > end
    {
        return invalid(offset);
    }
    Ok(completed)
}

fn parse_quantifier(pattern: &str, start: usize) -> Result<(String, usize), GrepCompileError> {
    let mut cursor = start + 1;
    if !pattern[cursor..].contains('}') {
        return invalid(start);
    }
    if pattern[cursor..].starts_with(',') {
        return unsupported("brace-spelling", start);
    }
    let first = cursor;
    while cursor < pattern.len() && pattern.as_bytes()[cursor].is_ascii_digit() {
        cursor += 1;
    }
    if cursor == first {
        return unsupported("brace-spelling", start);
    }
    let minimum = pattern[first..cursor]
        .parse::<u32>()
        .map_err(|_| GrepCompileError::InvalidPattern { offset: start })?;
    let mut maximum = None;
    if pattern[cursor..].starts_with(',') {
        cursor += 1;
        let maximum_start = cursor;
        while cursor < pattern.len() && pattern.as_bytes()[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor != maximum_start {
            maximum = Some(
                pattern[maximum_start..cursor]
                    .parse::<u32>()
                    .map_err(|_| GrepCompileError::InvalidPattern { offset: start })?,
            );
        }
    }
    if !pattern[cursor..].starts_with('}') {
        return invalid(start);
    }
    if maximum.is_some_and(|maximum| minimum > maximum) {
        return invalid(start);
    }
    let mut end = cursor + 1;
    if pattern[end..].starts_with('+') {
        return unsupported("possessive-quantifier", end);
    }
    if pattern[end..].starts_with('?') {
        end += 1;
    }
    Ok((pattern[start..end].to_owned(), end))
}

fn validate_character_escape(
    pattern: &str,
    offset: usize,
    escaped: char,
    after: usize,
) -> Result<(), GrepCompileError> {
    let (required_digits, braced_name) = match escaped {
        'x' => (2, false),
        'u' => (4, false),
        'U' => (8, false),
        'N' => (0, true),
        _ => unreachable!("only character-escape prefixes call this helper"),
    };
    if braced_name {
        let Some(rest) = pattern.get(after..) else {
            return invalid(offset);
        };
        let Some(name_end) = rest.find('}') else {
            return invalid(offset);
        };
        if !rest.starts_with('{') || name_end == 1 {
            return invalid(offset);
        }
        let name = &rest[1..name_end];
        if unicode_names2::character(name).is_none() {
            return invalid(offset);
        }
        return Ok(());
    }
    let Some(rest) = pattern.get(after..) else {
        return invalid(offset);
    };
    let candidate = rest.as_bytes().get(..required_digits);
    if candidate.is_none_or(|digits| !digits.iter().all(u8::is_ascii_hexdigit)) {
        return invalid(offset);
    }
    if escaped == 'U' {
        let candidate = candidate.expect("candidate checked above");
        let Ok(candidate) = std::str::from_utf8(candidate) else {
            return invalid(offset);
        };
        let Ok(scalar) = u32::from_str_radix(candidate, 16) else {
            return invalid(offset);
        };
        if scalar > 0x10ffff {
            return invalid(offset);
        }
    }
    Ok(())
}

fn next_scalar(pattern: &str, offset: usize) -> Result<(char, usize), GrepCompileError> {
    let scalar = pattern[offset..]
        .chars()
        .next()
        .ok_or(GrepCompileError::InvalidPattern { offset })?;
    Ok((scalar, offset + scalar.len_utf8()))
}

fn unsupported<T>(family: &'static str, offset: usize) -> Result<T, GrepCompileError> {
    Err(GrepCompileError::UnsupportedFamily { family, offset })
}

fn invalid<T>(offset: usize) -> Result<T, GrepCompileError> {
    Err(GrepCompileError::InvalidPattern { offset })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizer_rejections_never_reach_the_native_builder() {
        for pattern in [
            "\\0",
            "\\9",
            "\\s",
            "\\b",
            "\\n",
            "\\x41",
            "\\u0041",
            "\\U0001F642",
            "\\N{BULLET}",
            "\\N{NO SUCH UNICODE NAME}",
            "(?m)foo",
            "(?ii)foo",
            "(?-ii:foo)",
            "(?i-i:foo)",
            "a(?i)",
            "a(?i:b)",
            "(?P<n>a)",
            "(?P<a\u{301}>a)",
            "(?P<℘>a)",
            "(?P<a·b>a)",
            "(?P<1a>a)",
            "(?P<a-b>a)",
            "(?P<a\u{200c}>a)",
            "(?P<a\u{200d}>a)",
            "(?P=n)",
            "(?=a)",
            "(?!a)",
            "(?<=a)",
            "(?<!a)",
            "(?(1)a|b)",
            "(a)?(?(1)b|c)",
            "(?#note)",
            "(?>a)",
            "[a--b]",
            "[a&&b]",
            "[a~~b]",
            "[a||b]",
            "[[:alpha:]]",
            "[z-a]",
            "[a",
            "a{3,2}",
            "a{b}",
            "a{,3}",
            "a++",
            "foo$|bar",
            "foo\\Z|bar$",
            "(?R)",
            "(?U)",
            "(?L)",
            "(?<name>a)",
            "\\p{L}",
            "\\R",
            "\\c",
            "(?P<",
            "(?P<n>",
            "(?P=",
            "(?=",
            "(?<=",
            "(?>",
            "(?:(?i))",
            "(a",
            "]",
            "*a",
            "a{2",
            "\\x4",
            "é\\c(?i)",
        ] {
            let mut calls = 0;
            let result = compile_grep_pattern_with(pattern, &mut |translated| {
                calls += 1;
                Regex::new(translated)
            });
            assert!(result.is_err(), "{pattern:?}");
            assert_eq!(calls, 0, "{pattern:?}");
        }
    }

    #[test]
    fn admitted_and_translated_patterns_compile_exactly_once() {
        for pattern in [
            "",
            "literal",
            "café🙂",
            ".",
            "^foo",
            "a|b",
            "(a)",
            "(?:a|b)",
            "\\.",
            "\\é",
            "[a-z]",
            "[-a]",
            "[a-]",
            "[a-b-a]",
            "[a-b-c]",
            "\\d+",
            "\\D",
            "[\\dA-Z]",
            "a*",
            "a+",
            "a?",
            "a*?",
            "a{2}",
            "a{2,}",
            "a{2,3}",
            "(?:a|b){2,3}?",
            "\\Afoo",
            "foo\\Z",
            "foo\\z",
            "foo$",
        ] {
            let mut calls = 0;
            let compiled = compile_grep_pattern_with(pattern, &mut |translated| {
                calls += 1;
                Regex::new(translated)
            })
            .unwrap_or_else(|error| panic!("{pattern:?}: {error:?}"));
            assert_eq!(calls, 1, "{pattern:?}");
            if pattern == "foo$" {
                assert!(compiled.is_match("foo"));
                assert!(compiled.is_match("foo\n"));
                assert!(!compiled.is_match("foo\n\n"));
                assert_eq!(calls, 1, "search retries must not rebuild");
            }
        }
    }

    #[test]
    fn native_compile_failure_is_reserved_for_the_injected_builder() {
        let mut calls = 0;
        let error = compile_grep_pattern_with("valid", &mut |_| {
            calls += 1;
            let invalid = String::from("[");
            Regex::new(&invalid)
        })
        .expect_err("injected builder failure must surface");
        assert_eq!(calls, 1);
        assert_eq!(error, GrepCompileError::NativeCompileFailure);
    }
}
