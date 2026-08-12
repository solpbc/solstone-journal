// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A deliberately small portable-JSON recognizer.
//!
//! It is iterative so a hostile nested row cannot consume the Rust stack. It
//! accepts Python's three non-standard constants solely to classify their
//! documented compatibility behavior; every other grammar difference fails
//! closed as `Unknown`.

use crate::unicode::is_python_whitespace;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyntaxClass<'a> {
    Valid(RowFacts<'a>),
    Malformed,
    IntegerDigitLimit,
    RecursionLimit,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RowFacts<'a> {
    pub top_is_object: bool,
    pub ts: Option<Value<'a>>,
    pub max_depth: usize,
    pub has_lone_surrogate: bool,
    pub has_integer_digit_limit: bool,
    pub has_extended_outside_last_ts: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Value<'a> {
    String(&'a [u8]),
    Number(&'a [u8]),
    Bool(bool),
    Null,
    Container,
    Extended(Extended),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Extended {
    Nan,
    PositiveInfinity,
    NegativeInfinity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    OpenObject,
    CloseObject,
    OpenArray,
    CloseArray,
    Colon,
    Comma,
    String { lone_surrogate: bool },
    Number { integer_too_long: bool },
    True,
    False,
    Null,
    Extended(Extended),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LexError {
    Malformed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ScanLimits {
    first_integer_digit_limit: Option<usize>,
    first_recursion_limit: Option<usize>,
}

#[derive(Debug)]
struct Lexed {
    tokens: Vec<Token>,
    limits: ScanLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LexFailure {
    error: LexError,
    at: usize,
    limits: ScanLimits,
}

fn json_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn parse_string(bytes: &[u8], start: usize) -> Result<(usize, bool), LexError> {
    let mut index = start + 1;
    let mut lone_surrogate = false;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => return Ok((index + 1, lone_surrogate)),
            0x00..=0x1f => return Err(LexError::Unknown),
            b'\\' => {
                index += 1;
                let Some(&escape) = bytes.get(index) else {
                    return Err(LexError::Unknown);
                };
                if escape != b'u' {
                    if !matches!(
                        escape,
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't'
                    ) {
                        return Err(LexError::Unknown);
                    }
                    index += 1;
                    continue;
                }
                let Some(hex) = bytes.get(index + 1..index + 5) else {
                    return Err(LexError::Unknown);
                };
                let mut value = 0_u16;
                for digit in hex {
                    value = value
                        .checked_mul(16)
                        .and_then(|current| match digit {
                            b'0'..=b'9' => Some(current + u16::from(digit - b'0')),
                            b'a'..=b'f' => Some(current + u16::from(digit - b'a' + 10)),
                            b'A'..=b'F' => Some(current + u16::from(digit - b'A' + 10)),
                            _ => None,
                        })
                        .ok_or(LexError::Unknown)?;
                }
                index += 5;
                if (0xd800..=0xdbff).contains(&value) {
                    let paired = bytes.get(index..index + 6).is_some_and(|pair| {
                        pair[0] == b'\\'
                            && pair[1] == b'u'
                            && decode_hex_u16(&pair[2..6])
                                .is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
                    });
                    if paired {
                        index += 6;
                    } else {
                        lone_surrogate = true;
                    }
                } else if (0xdc00..=0xdfff).contains(&value) {
                    lone_surrogate = true;
                }
            }
            _ => index += 1,
        }
    }
    Err(LexError::Unknown)
}

fn decode_hex_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    let mut value = 0_u16;
    for digit in bytes {
        value = value.checked_mul(16)?
            + match digit {
                b'0'..=b'9' => u16::from(digit - b'0'),
                b'a'..=b'f' => u16::from(digit - b'a' + 10),
                b'A'..=b'F' => u16::from(digit - b'A' + 10),
                _ => return None,
            };
    }
    Some(value)
}

fn number_prefix(bytes: &[u8], start: usize) -> Option<(usize, bool)> {
    let mut index = start;
    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }
    let integer_start = index;
    match bytes.get(index)? {
        b'0' => index += 1,
        b'1'..=b'9' => {
            index += 1;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
        }
        _ => return None,
    }
    let integer_end = index;
    let mut integral = true;
    if bytes.get(index) == Some(&b'.') && bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
        integral = false;
        index += 2;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        let mut exponent = index + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            integral = false;
            index = exponent + 1;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
        }
    }
    Some((index, integral && integer_end - integer_start > 4300))
}

fn keyword_prefix(bytes: &[u8], index: usize) -> Option<(usize, TokenKind)> {
    let remainder = &bytes[index..];
    [
        (
            b"-Infinity".as_slice(),
            TokenKind::Extended(Extended::NegativeInfinity),
        ),
        (
            b"Infinity".as_slice(),
            TokenKind::Extended(Extended::PositiveInfinity),
        ),
        (b"false".as_slice(), TokenKind::False),
        (b"true".as_slice(), TokenKind::True),
        (b"null".as_slice(), TokenKind::Null),
        (b"NaN".as_slice(), TokenKind::Extended(Extended::Nan)),
    ]
    .into_iter()
    .find_map(|(literal, kind)| {
        remainder
            .starts_with(literal)
            .then_some((literal.len(), kind))
    })
}

fn lex(bytes: &[u8]) -> Result<Lexed, LexFailure> {
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut open_containers = 0_usize;
    let mut limits = ScanLimits::default();
    while index < bytes.len() {
        if json_space(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        let kind = match bytes[index] {
            b'{' => {
                index += 1;
                TokenKind::OpenObject
            }
            b'}' => {
                index += 1;
                TokenKind::CloseObject
            }
            b'[' => {
                index += 1;
                TokenKind::OpenArray
            }
            b']' => {
                index += 1;
                TokenKind::CloseArray
            }
            b':' => {
                index += 1;
                TokenKind::Colon
            }
            b',' => {
                index += 1;
                TokenKind::Comma
            }
            b'"' => {
                let (end, lone_surrogate) =
                    parse_string(bytes, index).map_err(|error| LexFailure {
                        error,
                        at: start,
                        limits,
                    })?;
                index = end;
                TokenKind::String { lone_surrogate }
            }
            _ => {
                if let Some((length, kind)) = keyword_prefix(bytes, index) {
                    index += length;
                    kind
                } else if let Some((end, integer_too_long)) = number_prefix(bytes, index) {
                    index = end;
                    TokenKind::Number { integer_too_long }
                } else {
                    return Err(LexFailure {
                        error: LexError::Malformed,
                        at: start,
                        limits,
                    });
                }
            }
        };
        match kind {
            TokenKind::OpenObject | TokenKind::OpenArray => {
                open_containers += 1;
                if open_containers >= 10_001 && limits.first_recursion_limit.is_none() {
                    limits.first_recursion_limit = Some(start);
                }
            }
            TokenKind::CloseObject | TokenKind::CloseArray => {
                open_containers = open_containers.saturating_sub(1);
            }
            TokenKind::Number {
                integer_too_long: true,
            } => {
                limits.first_integer_digit_limit.get_or_insert(start);
            }
            _ => {}
        }
        tokens.push(Token {
            kind,
            start,
            end: index,
        });
    }
    Ok(Lexed { tokens, limits })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    NeedValue,
    Done { direct_extended: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    ObjectKey { after_comma: bool },
    ObjectColon { key_is_ts: bool },
    ObjectValue { key_is_ts: bool },
    ObjectComma,
    ArrayValue { after_comma: bool },
    ArrayComma,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Frame {
    state: State,
    start: usize,
}

fn value_from_token<'a>(token: Token, bytes: &'a [u8]) -> Option<Value<'a>> {
    Some(match token.kind {
        TokenKind::String { .. } => Value::String(&bytes[token.start..token.end]),
        TokenKind::Number { .. } => Value::Number(&bytes[token.start..token.end]),
        TokenKind::True => Value::Bool(true),
        TokenKind::False => Value::Bool(false),
        TokenKind::Null => Value::Null,
        TokenKind::Extended(value) => Value::Extended(value),
        _ => return None,
    })
}

fn string_is_ts(raw: &[u8]) -> bool {
    decode_string(raw).as_deref() == Some(&[u32::from(b't'), u32::from(b's')][..])
}

pub(crate) fn decode_string(raw: &[u8]) -> Option<Vec<u32>> {
    if raw.len() < 2 || raw.first() != Some(&b'"') || raw.last() != Some(&b'"') {
        return None;
    }
    let text = core::str::from_utf8(&raw[1..raw.len() - 1]).ok()?;
    let mut result = Vec::new();
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '\\' {
            result.push(chars[index] as u32);
            index += 1;
            continue;
        }
        index += 1;
        let escape = *chars.get(index)?;
        index += 1;
        match escape {
            '"' => result.push(u32::from(b'"')),
            '\\' => result.push(u32::from(b'\\')),
            '/' => result.push(u32::from(b'/')),
            'b' => result.push(8),
            'f' => result.push(12),
            'n' => result.push(10),
            'r' => result.push(13),
            't' => result.push(9),
            'u' => {
                let mut hex = [0_u8; 4];
                for item in &mut hex {
                    *item = (*chars.get(index)? as u32).try_into().ok()?;
                    index += 1;
                }
                let high = decode_hex_u16(&hex)?;
                if (0xd800..=0xdbff).contains(&high)
                    && chars.get(index) == Some(&'\\')
                    && chars.get(index + 1) == Some(&'u')
                {
                    let mut low_hex = [0_u8; 4];
                    for (offset, item) in low_hex.iter_mut().enumerate() {
                        *item = (*chars.get(index + 2 + offset)? as u32).try_into().ok()?;
                    }
                    if let Some(low) = decode_hex_u16(&low_hex)
                        && (0xdc00..=0xdfff).contains(&low)
                    {
                        result.push(
                            0x1_0000 + ((u32::from(high - 0xd800)) << 10) + u32::from(low - 0xdc00),
                        );
                        index += 6;
                        continue;
                    }
                }
                result.push(u32::from(high));
            }
            _ => return None,
        }
    }
    Some(result)
}

fn failure_after_prefix<'a>(
    limits: ScanLimits,
    at: usize,
    fallback: SyntaxClass<'a>,
) -> SyntaxClass<'a> {
    if limits
        .first_integer_digit_limit
        .is_some_and(|limit| limit < at)
    {
        SyntaxClass::IntegerDigitLimit
    } else if limits.first_recursion_limit.is_some_and(|limit| limit < at) {
        SyntaxClass::RecursionLimit
    } else {
        fallback
    }
}

/// Classifies a nonblank, UTF-8 row. Callers apply byte and UTF-8 precedence.
pub(crate) fn recognize<'a>(bytes: &'a [u8]) -> SyntaxClass<'a> {
    let text = match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return SyntaxClass::Unknown,
    };
    let trimmed = text.trim_matches(|character| is_python_whitespace(character as u32));
    let left = trimmed.as_ptr() as usize - text.as_ptr() as usize;
    let trimmed_bytes = &bytes[left..left + trimmed.len()];
    let Lexed { tokens, limits } = match lex(trimmed_bytes) {
        Ok(lexed) => lexed,
        Err(LexFailure { error, at, limits }) => {
            let fallback = match error {
                LexError::Malformed => SyntaxClass::Malformed,
                LexError::Unknown => SyntaxClass::Unknown,
            };
            return failure_after_prefix(limits, at, fallback);
        }
    };
    if tokens.is_empty() {
        return SyntaxClass::Malformed;
    }

    let mut root = Root::NeedValue;
    let mut frames = Vec::<Frame>::new();
    let mut ts = None;
    let mut max_depth = 0_usize;
    let mut has_lone_surrogate = false;
    let mut has_integer_digit_limit = false;
    let mut extended = Vec::<(usize, usize)>::new();

    macro_rules! parse_step {
        ($expression:expr, $at:expr) => {
            if let Err(class) = $expression {
                return failure_after_prefix(limits, $at, class);
            }
        };
    }

    for token in tokens {
        if let TokenKind::String { lone_surrogate } = token.kind {
            has_lone_surrogate |= lone_surrogate;
        }
        if let TokenKind::Number { integer_too_long } = token.kind {
            has_integer_digit_limit |= integer_too_long;
        }
        if matches!(token.kind, TokenKind::Extended(_)) {
            extended.push((token.start, token.end));
        }
        let current = frames.last().map(|frame| frame.state);
        match current {
            None => match root {
                Root::NeedValue => parse_step!(
                    start_value(
                        token,
                        &mut root,
                        &mut frames,
                        &mut max_depth,
                        None,
                        &mut ts,
                        trimmed_bytes
                    ),
                    token.start
                ),
                Root::Done { .. } => {
                    return failure_after_prefix(limits, token.start, SyntaxClass::Malformed);
                }
            },
            Some(State::ObjectKey { after_comma }) => match token.kind {
                TokenKind::CloseObject if !after_comma => parse_step!(
                    close_container(token, &mut root, &mut frames, &mut ts, trimmed_bytes),
                    token.start
                ),
                TokenKind::CloseObject => {
                    return failure_after_prefix(limits, token.start, SyntaxClass::Malformed);
                }
                TokenKind::String { .. } => {
                    let key_is_ts = string_is_ts(&trimmed_bytes[token.start..token.end]);
                    frames.last_mut().expect("frame exists").state =
                        State::ObjectColon { key_is_ts };
                }
                _ => return failure_after_prefix(limits, token.start, SyntaxClass::Unknown),
            },
            Some(State::ObjectColon { key_is_ts }) => {
                if token.kind != TokenKind::Colon {
                    return failure_after_prefix(limits, token.start, SyntaxClass::Unknown);
                }
                frames.last_mut().expect("frame exists").state = State::ObjectValue { key_is_ts };
            }
            Some(State::ObjectValue { key_is_ts }) => {
                parse_step!(
                    start_value(
                        token,
                        &mut root,
                        &mut frames,
                        &mut max_depth,
                        Some(key_is_ts),
                        &mut ts,
                        trimmed_bytes
                    ),
                    token.start
                );
            }
            Some(State::ObjectComma) => match token.kind {
                TokenKind::Comma => {
                    frames.last_mut().expect("frame exists").state =
                        State::ObjectKey { after_comma: true }
                }
                TokenKind::CloseObject => parse_step!(
                    close_container(token, &mut root, &mut frames, &mut ts, trimmed_bytes),
                    token.start
                ),
                _ => return failure_after_prefix(limits, token.start, SyntaxClass::Malformed),
            },
            Some(State::ArrayValue { after_comma }) => match token.kind {
                TokenKind::CloseArray if !after_comma => parse_step!(
                    close_container(token, &mut root, &mut frames, &mut ts, trimmed_bytes),
                    token.start
                ),
                TokenKind::CloseArray => {
                    return failure_after_prefix(limits, token.start, SyntaxClass::Malformed);
                }
                _ => parse_step!(
                    start_value(
                        token,
                        &mut root,
                        &mut frames,
                        &mut max_depth,
                        None,
                        &mut ts,
                        trimmed_bytes
                    ),
                    token.start
                ),
            },
            Some(State::ArrayComma) => match token.kind {
                TokenKind::Comma => {
                    frames.last_mut().expect("frame exists").state =
                        State::ArrayValue { after_comma: true }
                }
                TokenKind::CloseArray => parse_step!(
                    close_container(token, &mut root, &mut frames, &mut ts, trimmed_bytes),
                    token.start
                ),
                _ => return failure_after_prefix(limits, token.start, SyntaxClass::Malformed),
            },
        }
    }
    let Root::Done { direct_extended } = root else {
        return failure_after_prefix(limits, trimmed_bytes.len(), SyntaxClass::Unknown);
    };
    if !frames.is_empty() {
        return failure_after_prefix(limits, trimmed_bytes.len(), SyntaxClass::Unknown);
    }
    let object = trimmed_bytes.first() == Some(&b'{');
    let last_ts_span = ts.map(|(_, start, end)| (start, end));
    let has_extended_outside_last_ts = !extended.is_empty()
        && !(object && extended.iter().all(|span| Some(*span) == last_ts_span)
            || (!object && direct_extended));
    SyntaxClass::Valid(RowFacts {
        top_is_object: object,
        ts: ts.map(|(value, _, _)| value),
        max_depth,
        has_lone_surrogate,
        has_integer_digit_limit,
        has_extended_outside_last_ts,
    })
}

fn start_value<'a>(
    token: Token,
    root: &mut Root,
    frames: &mut Vec<Frame>,
    max_depth: &mut usize,
    key_is_ts: Option<bool>,
    ts: &mut Option<(Value<'a>, usize, usize)>,
    bytes: &'a [u8],
) -> Result<(), SyntaxClass<'a>> {
    match token.kind {
        TokenKind::OpenObject => {
            *max_depth = (*max_depth).max(frames.len() + 1);
            frames.push(Frame {
                state: State::ObjectKey { after_comma: false },
                start: token.start,
            });
        }
        TokenKind::OpenArray => {
            *max_depth = (*max_depth).max(frames.len() + 1);
            frames.push(Frame {
                state: State::ArrayValue { after_comma: false },
                start: token.start,
            });
        }
        TokenKind::CloseObject | TokenKind::CloseArray | TokenKind::Colon | TokenKind::Comma => {
            return Err(SyntaxClass::Malformed);
        }
        _ => {
            let value = value_from_token(token, bytes).ok_or(SyntaxClass::Unknown)?;
            complete_value(root, frames, key_is_ts, value, token.start, token.end, ts);
        }
    }
    Ok(())
}

fn close_container<'a>(
    token: Token,
    root: &mut Root,
    frames: &mut Vec<Frame>,
    ts: &mut Option<(Value<'a>, usize, usize)>,
    _bytes: &'a [u8],
) -> Result<(), SyntaxClass<'a>> {
    let frame = frames.pop().ok_or(SyntaxClass::Unknown)?;
    let matches = matches!(
        (frame.state, token.kind),
        (
            State::ObjectKey { .. } | State::ObjectComma,
            TokenKind::CloseObject
        ) | (
            State::ArrayValue { .. } | State::ArrayComma,
            TokenKind::CloseArray
        )
    );
    if !matches {
        return Err(SyntaxClass::Unknown);
    }
    let parent_ts = matches!(
        frames.last().map(|parent| parent.state),
        Some(State::ObjectValue { key_is_ts: true })
    );
    complete_value(
        root,
        frames,
        Some(parent_ts),
        Value::Container,
        frame.start,
        token.end,
        ts,
    );
    Ok(())
}

fn complete_value<'a>(
    root: &mut Root,
    frames: &mut [Frame],
    key_is_ts: Option<bool>,
    value: Value<'a>,
    start: usize,
    end: usize,
    ts: &mut Option<(Value<'a>, usize, usize)>,
) {
    let frame_count = frames.len();
    if let Some(parent) = frames.last_mut() {
        if matches!(parent.state, State::ObjectValue { .. }) {
            // `dict.get("ts")` only observes the outer decoded object. Nested
            // objects can of course have their own `ts` members, but they are
            // unrelated to steward's explicit coercion.
            if frame_count == 1 && key_is_ts == Some(true) {
                *ts = Some((value, start, end));
            }
            parent.state = State::ObjectComma;
        } else if matches!(parent.state, State::ArrayValue { .. }) {
            parent.state = State::ArrayComma;
        }
        return;
    }
    *root = Root::Done {
        direct_extended: matches!(value, Value::Extended(_)),
    };
}
