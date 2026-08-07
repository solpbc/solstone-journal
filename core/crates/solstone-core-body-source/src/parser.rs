// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use crate::{BodyInteger, BodyString, BodyValue, ParseError};

pub(crate) const MAX_NESTING: usize = 128;

/// Decodes exactly one Python-3.12-compatible JSON text from UTF-8 bytes.
pub fn parse(bytes: &[u8]) -> Result<BodyValue, ParseError> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| ParseError::malformed(error.valid_up_to()))?;
    let mut parser = Parser {
        text,
        byte_offset: 0,
        depth: 0,
    };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.byte_offset != parser.text.len() {
        return Err(parser.malformed_here());
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    byte_offset: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn parse_value(&mut self) -> Result<BodyValue, ParseError> {
        let start = self.byte_offset;
        match self.byte_at(self.byte_offset) {
            Some(b'"') => self.parse_string().map(BodyValue::String),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'n') => self.parse_literal("null", BodyValue::Null),
            Some(b't') => self.parse_literal("true", BodyValue::Bool(true)),
            Some(b'f') => self.parse_literal("false", BodyValue::Bool(false)),
            Some(b'N') => self.parse_literal("NaN", BodyValue::Number(f64::NAN)),
            Some(b'I') => self.parse_literal("Infinity", BodyValue::Number(f64::INFINITY)),
            Some(byte) if byte.is_ascii_digit() || byte == b'-' => {
                if let Some(number) = self.parse_number()? {
                    Ok(number)
                } else if byte == b'-' {
                    self.parse_literal("-Infinity", BodyValue::Number(f64::NEG_INFINITY))
                } else {
                    Err(ParseError::malformed(start))
                }
            }
            _ => Err(ParseError::malformed(start)),
        }
    }

    fn parse_literal(&mut self, literal: &str, value: BodyValue) -> Result<BodyValue, ParseError> {
        let start = self.byte_offset;
        if self.text[self.byte_offset..].starts_with(literal) {
            self.byte_offset += literal.len();
            Ok(value)
        } else {
            Err(ParseError::malformed(start))
        }
    }

    fn parse_number(&mut self) -> Result<Option<BodyValue>, ParseError> {
        let start = self.byte_offset;
        let mut cursor = start;
        let negative = self.byte_at(cursor) == Some(b'-');
        if negative {
            cursor += 1;
        }
        let digit_start = cursor;
        let Some(first_digit) = self.byte_at(cursor) else {
            return Ok(None);
        };
        if !first_digit.is_ascii_digit() {
            return Ok(None);
        }

        let digit_count = if first_digit == b'0' {
            cursor += 1;
            1
        } else {
            cursor += 1;
            let mut count = 1usize;
            while self
                .byte_at(cursor)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                cursor += 1;
                count += 1;
            }
            count
        };

        let mut is_float = false;
        if self.byte_at(cursor) == Some(b'.')
            && self
                .byte_at(cursor + 1)
                .is_some_and(|byte| byte.is_ascii_digit())
        {
            is_float = true;
            cursor += 2;
            while self
                .byte_at(cursor)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                cursor += 1;
            }
        }

        if matches!(self.byte_at(cursor), Some(b'e' | b'E')) {
            let mut exponent = cursor + 1;
            if matches!(self.byte_at(exponent), Some(b'+' | b'-')) {
                exponent += 1;
            }
            if self
                .byte_at(exponent)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                is_float = true;
                cursor = exponent + 1;
                while self
                    .byte_at(cursor)
                    .is_some_and(|byte| byte.is_ascii_digit())
                {
                    cursor += 1;
                }
            }
        }

        if is_float {
            let number = self.text[start..cursor]
                .parse::<f64>()
                .map_err(|_| ParseError::malformed(start))?;
            self.byte_offset = cursor;
            return Ok(Some(BodyValue::Number(number)));
        }

        if digit_count > 4300 {
            return Err(ParseError::NumberTooLong {
                byte_offset: digit_start + 4300,
            });
        }
        let digits = &self.text[digit_start..cursor];
        let integer =
            BodyInteger::new(negative, digits).expect("number scanner produced normalized digits");
        self.byte_offset = cursor;
        Ok(Some(BodyValue::Integer(integer)))
    }

    fn parse_string(&mut self) -> Result<BodyString, ParseError> {
        self.byte_offset += 1;
        let mut code_points = Vec::new();
        loop {
            let Some(byte) = self.byte_at(self.byte_offset) else {
                return Err(self.malformed_here());
            };
            match byte {
                b'"' => {
                    self.byte_offset += 1;
                    return Ok(BodyString::from_decoded(code_points));
                }
                b'\\' => self.parse_escape(&mut code_points)?,
                0..=0x1f => return Err(self.malformed_here()),
                _ if byte.is_ascii() => {
                    code_points.push(u32::from(byte));
                    self.byte_offset += 1;
                }
                _ => {
                    let character = self.text[self.byte_offset..]
                        .chars()
                        .next()
                        .expect("validated UTF-8 has a character at this offset");
                    code_points.push(u32::from(character));
                    self.byte_offset += character.len_utf8();
                }
            }
        }
    }

    fn parse_escape(&mut self, code_points: &mut Vec<u32>) -> Result<(), ParseError> {
        let escape_start = self.byte_offset;
        self.byte_offset += 1;
        let Some(escape) = self.byte_at(self.byte_offset) else {
            return Err(ParseError::malformed(escape_start));
        };
        match escape {
            b'"' => code_points.push(u32::from(b'"')),
            b'\\' => code_points.push(u32::from(b'\\')),
            b'/' => code_points.push(u32::from(b'/')),
            b'b' => code_points.push(u32::from(b'\x08')),
            b'f' => code_points.push(u32::from(b'\x0c')),
            b'n' => code_points.push(u32::from(b'\n')),
            b'r' => code_points.push(u32::from(b'\r')),
            b't' => code_points.push(u32::from(b'\t')),
            b'u' => {
                let unicode_start = self.byte_offset;
                self.byte_offset += 1;
                let code_point = self.decode_unicode_escape(unicode_start)?;
                if (0xd800..=0xdbff).contains(&code_point)
                    && self.byte_at(self.byte_offset) == Some(b'\\')
                    && self.byte_at(self.byte_offset + 1) == Some(b'u')
                    && let Some(low) = self.peek_unicode_escape(self.byte_offset + 2)
                    && (0xdc00..=0xdfff).contains(&low)
                {
                    self.byte_offset += 2 + 4;
                    code_points.push(0x10000 + (code_point - 0xd800) * 0x400 + (low - 0xdc00));
                } else {
                    code_points.push(code_point);
                }
                return Ok(());
            }
            _ => return Err(ParseError::malformed(escape_start)),
        }
        self.byte_offset += 1;
        Ok(())
    }

    fn decode_unicode_escape(&mut self, unicode_start: usize) -> Result<u32, ParseError> {
        let Some(code_point) = self.peek_unicode_escape(self.byte_offset) else {
            return Err(ParseError::malformed(unicode_start));
        };
        self.byte_offset += 4;
        Ok(code_point)
    }

    fn peek_unicode_escape(&self, start: usize) -> Option<u32> {
        let bytes = self.text.as_bytes().get(start..start + 4)?;
        let mut code_point = 0u32;
        for byte in bytes {
            code_point = code_point.checked_mul(16)? + u32::from(hex_value(*byte)?);
        }
        Some(code_point)
    }

    fn parse_array(&mut self) -> Result<BodyValue, ParseError> {
        let opener = self.byte_offset;
        self.enter_container(opener)?;
        self.byte_offset += 1;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.byte_at(self.byte_offset) == Some(b']') {
            self.byte_offset += 1;
            self.depth -= 1;
            return Ok(BodyValue::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            match self.byte_at(self.byte_offset) {
                Some(b',') => {
                    self.byte_offset += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.byte_offset += 1;
                    self.depth -= 1;
                    return Ok(BodyValue::Array(values));
                }
                _ => return Err(self.malformed_here()),
            }
        }
    }

    fn parse_object(&mut self) -> Result<BodyValue, ParseError> {
        let opener = self.byte_offset;
        self.enter_container(opener)?;
        self.byte_offset += 1;
        self.skip_whitespace();
        let mut object = BTreeMap::new();
        if self.byte_at(self.byte_offset) == Some(b'}') {
            self.byte_offset += 1;
            self.depth -= 1;
            return Ok(BodyValue::Object(object));
        }
        loop {
            if self.byte_at(self.byte_offset) != Some(b'"') {
                return Err(self.malformed_here());
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.byte_at(self.byte_offset) != Some(b':') {
                return Err(self.malformed_here());
            }
            self.byte_offset += 1;
            self.skip_whitespace();
            let value = self.parse_value()?;
            object.insert(key, value);
            self.skip_whitespace();
            match self.byte_at(self.byte_offset) {
                Some(b',') => {
                    self.byte_offset += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.byte_offset += 1;
                    self.depth -= 1;
                    return Ok(BodyValue::Object(object));
                }
                _ => return Err(self.malformed_here()),
            }
        }
    }

    fn enter_container(&mut self, opener: usize) -> Result<(), ParseError> {
        if self.depth >= MAX_NESTING {
            return Err(ParseError::malformed(opener));
        }
        self.depth += 1;
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while matches!(
            self.byte_at(self.byte_offset),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.byte_offset += 1;
        }
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        self.text.as_bytes().get(offset).copied()
    }

    fn malformed_here(&self) -> ParseError {
        ParseError::malformed(self.byte_offset)
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
