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
    let mut output = String::with_capacity(pattern.len());
    let mut cursor = 0;
    let mut depth = 0_usize;
    let mut retry_without_final_lf = false;
    let mut seen_dollar = false;
    let mut seen_exact_end_anchor = false;
    let mut can_quantify = false;
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
                    '1'..='9' => return unsupported("backreference", cursor),
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
            }
            '[' => {
                let (class, after) = translate_class(pattern, cursor)?;
                output.push_str(&class);
                cursor = after;
                can_quantify = true;
            }
            ']' => return invalid(cursor),
            '(' => {
                if pattern[next..].starts_with('?') {
                    let offset = cursor;
                    let rest = &pattern[next + 1..];
                    if rest.starts_with("-u:") {
                        return invalid(offset);
                    }
                    if rest.starts_with("P<") {
                        return unsupported("named-group", offset);
                    }
                    if rest.starts_with("P=") {
                        return unsupported("backreference", offset);
                    }
                    if rest.starts_with('=')
                        || rest.starts_with('!')
                        || rest.starts_with("<=")
                        || rest.starts_with("<!")
                    {
                        return unsupported("lookaround", offset);
                    }
                    if rest.starts_with('>') {
                        return unsupported("atomic-group", offset);
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
                        depth += 1;
                        can_quantify = false;
                        continue;
                    }
                    if matches!(rest.chars().next(), Some('i' | 'x' | 'a' | 'u')) {
                        return unsupported("inline-flags", offset);
                    }
                    return invalid(offset);
                }
                output.push('(');
                depth += 1;
                cursor = next;
                can_quantify = false;
            }
            ')' => {
                if depth == 0 {
                    return invalid(cursor);
                }
                depth -= 1;
                output.push(')');
                cursor = next;
                can_quantify = true;
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
            }
            '{' => {
                if !can_quantify {
                    return invalid(cursor);
                }
                let (quantifier, after) = parse_quantifier(pattern, cursor)?;
                output.push_str(&quantifier);
                cursor = after;
            }
            '+' | '*' | '?' if pattern[next..].starts_with('+') => {
                return unsupported("possessive-quantifier", cursor);
            }
            _ => {
                output.push(scalar);
                cursor = next;
                can_quantify = !matches!(scalar, '^' | '|');
            }
        }
    }
    if depth != 0 {
        return invalid(pattern.len());
    }
    let regex = Regex::new(&output).map_err(|_| GrepCompileError::NativeCompileFailure)?;
    Ok(GrepPattern {
        regex,
        retry_without_final_lf,
    })
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
    while cursor < pattern.len() {
        let (scalar, next) = next_scalar(pattern, cursor)?;
        match scalar {
            ']' => {
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
            '&' if pattern[next..].starts_with('&') => {
                return unsupported("ambiguous-character-class", cursor);
            }
            '\\' => {
                let (escaped, after) = next_scalar(pattern, next)?;
                match escaped {
                    'd' => positive.push_str(&decimal_members()),
                    'D' => complement_decimal = true,
                    'b' | 'B' | 's' | 'S' | 'w' | 'W' => {
                        return unsupported("character-class-semantics", cursor);
                    }
                    'p' | 'P' | 'R' | 'A' | 'Z' | 'z' => return invalid(cursor),
                    '1'..='9' => return unsupported("backreference", cursor),
                    'a' | 'f' | 'n' | 'r' | 't' | 'v' => {
                        return unsupported("character-escape", cursor);
                    }
                    'x' | 'u' | 'U' | 'N' => {
                        validate_character_escape(pattern, cursor, escaped, after)?;
                        return unsupported("character-escape", cursor);
                    }
                    _ if escaped.is_ascii_alphabetic() => return invalid(cursor),
                    _ if !escaped.is_ascii() => positive.push(escaped),
                    _ => {
                        positive.push('\\');
                        positive.push(escaped);
                    }
                }
                cursor = after;
            }
            _ => {
                positive.push(scalar);
                cursor = next;
            }
        }
    }
    invalid(start)
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
    if pattern[cursor..].starts_with(',') {
        cursor += 1;
        while cursor < pattern.len() && pattern.as_bytes()[cursor].is_ascii_digit() {
            cursor += 1;
        }
    }
    if !pattern[cursor..].starts_with('}') {
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
        if !rest.starts_with('{') || rest.len() == 1 || !rest[1..].contains('}') {
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
