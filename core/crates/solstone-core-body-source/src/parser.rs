// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use crate::{BodyInteger, BodyString, BodyValue, ParseError};

pub(crate) const MAX_NESTING: usize = 128;

/// Decodes exactly one Python-3.12-compatible JSON text from UTF-8 bytes.
pub fn parse(input: &[u8]) -> Result<BodyValue, ParseError> {
    parse_core(input, false).map(|(value, _keys)| value)
}

#[allow(dead_code)]
pub(crate) fn parse_with_top_level_keys(
    input: &[u8],
) -> Result<(BodyValue, Vec<BodyString>), ParseError> {
    parse_core(input, true)
}

fn parse_core(
    input: &[u8],
    observe_top_level_keys: bool,
) -> Result<(BodyValue, Vec<BodyString>), ParseError> {
    let text =
        std::str::from_utf8(input).map_err(|error| ParseError::malformed(error.valid_up_to()))?;
    let mut parser = Parser {
        text,
        byte_offset: 0,
        depth: 0,
        observe_top_level_keys,
        top_level_keys: Vec::new(),
    };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.byte_offset != parser.text.len() {
        return Err(parser.malformed_here());
    }
    Ok((value, parser.top_level_keys))
}

struct Parser<'a> {
    text: &'a str,
    byte_offset: usize,
    depth: usize,
    observe_top_level_keys: bool,
    top_level_keys: Vec<BodyString>,
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
            if self.observe_top_level_keys && self.depth == 1 {
                self.top_level_keys.push(key.clone());
            }
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

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::{parse, parse_core, parse_with_top_level_keys};
    use crate::{BodyString, BodyValue, ParseError, canonicalize};

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../core/fixtures")
            .join(name)
    }

    fn fixture(name: &str) -> Value {
        let text = std::fs::read_to_string(fixture_path(name)).expect("fixture should read");
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name} should parse: {error}"))
    }

    fn body_fixture(name: &str) -> BodyValue {
        parse(&std::fs::read(fixture_path(name)).expect("fixture should read"))
            .expect("fixture should parse")
    }

    fn object_field<'a>(value: &'a BodyValue, field: &str) -> &'a BodyValue {
        let BodyValue::Object(object) = value else {
            panic!("fixture value should be an object");
        };
        object
            .get(&body_string(field))
            .unwrap_or_else(|| panic!("fixture object should contain {field}"))
    }

    fn optional_object_field<'a>(value: &'a BodyValue, field: &str) -> Option<&'a BodyValue> {
        let BodyValue::Object(object) = value else {
            panic!("fixture value should be an object");
        };
        object.get(&body_string(field))
    }

    fn array_field<'a>(value: &'a BodyValue, field: &str) -> &'a [BodyValue] {
        let BodyValue::Array(array) = object_field(value, field) else {
            panic!("fixture field {field} should be an array");
        };
        array
    }

    fn body_string_field(value: &BodyValue, field: &str) -> String {
        let BodyValue::String(string) = object_field(value, field) else {
            panic!("fixture field {field} should be a string");
        };
        body_string_to_string(string)
    }

    fn body_string_to_string(string: &BodyString) -> String {
        string
            .code_points()
            .iter()
            .map(|code_point| char::from_u32(*code_point).expect("fixture strings are Unicode"))
            .collect()
    }

    fn body_u64_field(value: &BodyValue, field: &str) -> u64 {
        let BodyValue::Integer(integer) = object_field(value, field) else {
            panic!("fixture field {field} should be an integer");
        };
        integer
            .digits()
            .parse()
            .expect("fixture integer should fit in u64")
    }

    fn string_field<'a>(value: &'a Value, field: &str) -> &'a str {
        value[field].as_str().expect("fixture string field")
    }

    fn expand_body_pattern(pattern: &BodyValue) -> String {
        if let Some(BodyValue::String(prefix)) = optional_object_field(pattern, "prefix_repeat") {
            let count = body_u64_field(pattern, "repeat_count") as usize;
            return format!(
                "{}{}",
                body_string_to_string(prefix).repeat(count),
                body_string_field(pattern, "suffix_repeat").repeat(count)
            );
        }

        let prefix = optional_object_field(pattern, "prefix")
            .map(|value| match value {
                BodyValue::String(value) => body_string_to_string(value),
                _ => panic!("fixture prefix should be a string"),
            })
            .unwrap_or_default();
        let repeat = body_string_field(pattern, "repeat");
        let count = body_u64_field(pattern, "repeat_count") as usize;
        let suffix = optional_object_field(pattern, "suffix")
            .map(|value| match value {
                BodyValue::String(value) => body_string_to_string(value),
                _ => panic!("fixture suffix should be a string"),
            })
            .unwrap_or_default();
        format!("{prefix}{}{suffix}", repeat.repeat(count))
    }

    fn decode_hex(raw: &str) -> Vec<u8> {
        assert_eq!(raw.len() % 2, 0, "hex input must have even length");
        (0..raw.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&raw[index..index + 2], 16).expect("hex byte"))
            .collect()
    }

    fn named_input(name: impl Into<String>, input: impl Into<Vec<u8>>) -> (String, Vec<u8>) {
        (name.into(), input.into())
    }

    fn fixture_inputs() -> Vec<(String, Vec<u8>)> {
        let mut inputs = Vec::new();

        let python_vectors = body_fixture("body_source_python_json_vectors.json");
        for section in [
            "canonical_cases",
            "float_cases",
            "malformed_cases",
            "string_decode_cases",
        ] {
            for case in array_field(&python_vectors, section) {
                inputs.push(named_input(
                    format!("python {section}: {}", body_string_field(case, "name")),
                    body_string_field(case, "raw_json"),
                ));
            }
        }
        for case in array_field(&python_vectors, "long_numeric_cases") {
            inputs.push(named_input(
                format!("python long numeric: {}", body_string_field(case, "name")),
                expand_body_pattern(object_field(case, "raw_pattern")),
            ));
        }
        let policy = object_field(&python_vectors, "policy_cases");
        for case in array_field(policy, "invalid_utf8") {
            inputs.push(named_input(
                format!(
                    "python invalid UTF-8: {}",
                    body_string_field(case, "expected_error")
                ),
                decode_hex(&body_string_field(case, "raw_hex")),
            ));
        }
        inputs.push(named_input(
            "python policy: too deep",
            expand_body_pattern(object_field(
                object_field(policy, "too_deep"),
                "raw_pattern",
            )),
        ));
        for case in array_field(policy, "too_long_integers") {
            inputs.push(named_input(
                format!(
                    "python too-long integer: {}",
                    body_string_field(case, "expected_error")
                ),
                expand_body_pattern(object_field(case, "raw_pattern")),
            ));
        }

        let codec_rows = fixture("body_source_codec_rows.json");
        for case in codec_rows["rows"].as_array().expect("codec rows") {
            inputs.push(named_input(
                format!("codec row: {}", string_field(case, "name")),
                serde_json::to_string(&case["row"]).expect("row should serialize"),
            ));
        }

        let hash_vectors = body_fixture("body_source_hash_vectors.json");
        for case in array_field(&hash_vectors, "dedupe_cases") {
            inputs.push(named_input(
                format!("hash dedupe: {}", body_string_field(case, "name")),
                canonicalize(object_field(case, "identity")).expect("identity should canonicalize"),
            ));
        }
        for case in array_field(&hash_vectors, "value_cases") {
            inputs.push(named_input(
                format!("hash value: {}", body_string_field(case, "name")),
                canonicalize(object_field(case, "input")).expect("input should canonicalize"),
            ));
        }
        for case in array_field(&hash_vectors, "python_nonfinite_value_cases") {
            inputs.push(named_input(
                format!("hash nonfinite: {}", body_string_field(case, "name")),
                body_string_field(case, "input_value_literal"),
            ));
        }
        for case in array_field(&hash_vectors, "python_overflow_value_cases") {
            inputs.push(named_input(
                format!("hash overflow: {}", body_string_field(case, "name")),
                body_string_field(case, "input_numeric_literal"),
            ));
        }
        for case in array_field(&hash_vectors, "python_large_integer_value_cases") {
            let pattern = object_field(case, "decimal_pattern");
            inputs.push(named_input(
                format!("hash large integer: {}", body_string_field(case, "name")),
                format!(
                    "{}{}",
                    body_string_field(pattern, "leading"),
                    "0".repeat(body_u64_field(pattern, "trailing_zeros") as usize)
                ),
            ));
        }

        let native_bundle = fixture("body_source_native_bundle_v1.json");
        for case in native_bundle["cases"]
            .as_array()
            .expect("native bundle cases")
        {
            let name = string_field(case, "name");
            inputs.push(named_input(
                format!("native manifest: {name}"),
                serde_json::to_string(&case["manifest"]).expect("manifest should serialize"),
            ));
            for field in [
                "expected_envelope_jsonl",
                "expected_normalized_jsonl",
                "expected_ledger_jsonl",
            ] {
                for (line_number, line) in string_field(case, field)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .enumerate()
                {
                    inputs.push(named_input(
                        format!("native {name} {field} line {}", line_number + 1),
                        line,
                    ));
                }
            }
        }

        inputs
    }

    fn generated_corpus() -> Vec<(String, Vec<u8>)> {
        vec![
            named_input(
                "insignificant whitespace",
                b" \t\n { \"b\" : 1, \"a\" : 2 } \r".to_vec(),
            ),
            named_input("member order", br#"{"z":1,"a":2,"m":3}"#.to_vec()),
            named_input("empty object", b"{}".to_vec()),
            named_input("literal duplicate", br#"{"a":1,"a":2}"#.to_vec()),
            named_input("escaped duplicate", br#"{"a":1,"\u0061":2}"#.to_vec()),
            named_input("mixed duplicates", br#"{"a":1,"\u0061":2,"a":3}"#.to_vec()),
            named_input(
                "escaped key characters",
                br#"{"\/":1,"\\":2,"\"":3,"\b":4,"\f":5,"\n":6,"\r":7,"\t":8}"#.to_vec(),
            ),
            named_input(
                "astral and lone surrogate keys",
                br#"{"\ud83e\udec0":1,"\ud800":2}"#.to_vec(),
            ),
            named_input(
                "structural text in string",
                br#"{"a":"{\"b\":1,[2]}"}"#.to_vec(),
            ),
            named_input(
                "nested arrays and objects",
                br#"{"a":[{"b":[{"c":null}]}]}"#.to_vec(),
            ),
            named_input("NaN", b"NaN".to_vec()),
            named_input("Infinity", b"Infinity".to_vec()),
            named_input("negative Infinity", b"-Infinity".to_vec()),
            named_input("fixed lower exponent boundary", b"1e-4".to_vec()),
            named_input("scientific lower exponent boundary", b"1e-5".to_vec()),
            named_input("fixed upper exponent boundary", b"1e15".to_vec()),
            named_input("scientific upper exponent boundary", b"1e16".to_vec()),
            named_input("maximum finite", b"1.7976931348623157e308".to_vec()),
            named_input("4300-digit integer", format!("1{}", "0".repeat(4299))),
            named_input("4301-digit integer", format!("1{}", "0".repeat(4300))),
            named_input("invalid UTF-8", vec![b'\"', 0xff, b'\"']),
            named_input("malformed exponent", b"1e".to_vec()),
            named_input("malformed decimal", b"1.".to_vec()),
            named_input("malformed negative", b"-".to_vec()),
            named_input("malformed escape", b"\"\\x\"".to_vec()),
            named_input("incomplete Unicode escape", b"\"\\u12\"".to_vec()),
            named_input("missing object delimiter", b"{".to_vec()),
            named_input("missing object colon", br#"{"a" 1}"#.to_vec()),
            named_input("trailing array delimiter", b"[1,]".to_vec()),
            named_input("trailing object delimiter", br#"{"a":1,}"#.to_vec()),
            named_input("trailing data", b"{}[]".to_vec()),
        ]
    }

    fn body_string(value: &str) -> BodyString {
        BodyString::from_code_points(value.chars().map(u32::from).collect())
            .expect("valid code points")
    }

    fn body_string_from_code_points(code_points: Vec<u32>) -> BodyString {
        BodyString::from_code_points(code_points).expect("valid code points")
    }

    fn assert_body_value_bitwise_eq(actual: &BodyValue, expected: &BodyValue) {
        assert_body_value_at_path(actual, expected, "$");
    }

    fn assert_body_value_at_path(actual: &BodyValue, expected: &BodyValue, path: &str) {
        match (actual, expected) {
            (BodyValue::Number(actual), BodyValue::Number(expected)) => {
                assert_eq!(
                    actual.to_bits(),
                    expected.to_bits(),
                    "number bits differ at {path}"
                );
            }
            (BodyValue::Array(actual), BodyValue::Array(expected)) => {
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "array length differs at {path}"
                );
                for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    assert_body_value_at_path(actual, expected, &format!("{path}[{index}]"));
                }
            }
            (BodyValue::Object(actual), BodyValue::Object(expected)) => {
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "object size differs at {path}"
                );
                for (key, actual) in actual {
                    let expected = expected.get(key).unwrap_or_else(|| {
                        panic!("missing object key {:?} at {path}", key.code_points())
                    });
                    assert_body_value_at_path(
                        actual,
                        expected,
                        &format!("{path}[{:?}]", key.code_points()),
                    );
                }
            }
            _ => assert_eq!(actual, expected, "value differs at {path}"),
        }
    }

    fn assert_entry_points_agree(input: &[u8]) {
        let public = catch_unwind(AssertUnwindSafe(|| parse(input)));
        assert!(public.is_ok(), "public parse panicked for {input:?}");
        let observed = catch_unwind(AssertUnwindSafe(|| parse_with_top_level_keys(input)));
        assert!(
            observed.is_ok(),
            "top-level-key parse panicked for {input:?}"
        );

        match (
            public.expect("public parse already checked"),
            observed.expect("top-level-key parse already checked"),
        ) {
            (Ok(public), Ok((observed, _))) => assert_body_value_bitwise_eq(&public, &observed),
            (Err(public), Err(observed)) => assert_eq!(public, observed),
            (public, observed) => panic!(
                "entry points disagree for {input:?}: public={public:?}, observed={observed:?}"
            ),
        }
    }

    fn repeated_objects(depth: usize) -> (Vec<u8>, usize) {
        let mut text = String::new();
        let mut last_opener = 0;
        for _ in 0..depth {
            last_opener = text.len();
            text.push_str(r#"{"a":"#);
        }
        text.push_str("null");
        text.push_str(&"}".repeat(depth));
        (text.into_bytes(), last_opener)
    }

    fn alternating_containers(depth: usize) -> (Vec<u8>, usize) {
        let mut text = String::new();
        let mut last_opener = 0;
        for index in 0..depth {
            last_opener = text.len();
            if index % 2 == 0 {
                text.push_str(r#"{"a":"#);
            } else {
                text.push('[');
            }
        }
        text.push_str("null");
        for index in (0..depth).rev() {
            text.push(if index % 2 == 0 { '}' } else { ']' });
        }
        (text.into_bytes(), last_opener)
    }

    fn assert_exact_error(input: &[u8], expected: ParseError) {
        assert_eq!(parse(input), Err(expected));
        assert_eq!(parse_with_top_level_keys(input), Err(expected));
    }

    #[test]
    fn fixture_inputs_match_both_entry_points() {
        for (name, input) in fixture_inputs() {
            assert_entry_points_agree(&input);
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn generated_corpus_matches_both_entry_points() {
        for (name, input) in generated_corpus() {
            assert_entry_points_agree(&input);
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn generated_depth_limits_preserve_top_level_boundary() {
        for build in [
            repeated_objects as fn(usize) -> (Vec<u8>, usize),
            alternating_containers,
        ] {
            let (within_limit, _) = build(128);
            assert_entry_points_agree(&within_limit);
            let (_, keys) = parse_with_top_level_keys(&within_limit).expect("128 containers parse");
            assert_eq!(keys, vec![body_string("a")]);

            let (too_deep, offset) = build(129);
            assert_entry_points_agree(&too_deep);
            assert_exact_error(
                &too_deep,
                ParseError::MalformedJson {
                    byte_offset: offset,
                },
            );
        }
    }

    #[test]
    fn top_level_key_vectors_are_source_ordered_and_lossless() {
        let duplicate = br#"{"a":1,"a":2}"#;
        let (value, keys) = parse_with_top_level_keys(duplicate).expect("duplicate parses");
        assert_eq!(keys, vec![body_string("a"), body_string("a")]);
        assert_body_value_bitwise_eq(&value, &parse(br#"{"a":2}"#).expect("final object parses"));

        let (_, keys) =
            parse_with_top_level_keys(br#"{"a":1,"\u0061":2}"#).expect("decoded duplicate parses");
        assert_eq!(keys, vec![body_string("a"), body_string("a")]);

        let (_, keys) = parse_with_top_level_keys(br#"{"a":1,"\u0061":2,"a":3}"#)
            .expect("triple duplicate parses");
        assert_eq!(
            keys,
            vec![body_string("a"), body_string("a"), body_string("a")]
        );

        let (_, keys) = parse_with_top_level_keys(br#"{"b":1,"a":2}"#).expect("ordered keys parse");
        assert_eq!(keys, vec![body_string("b"), body_string("a")]);

        let (_, keys) = parse_with_top_level_keys(br#"{"":1}"#).expect("empty key parses");
        assert_eq!(keys, vec![body_string("")]);

        let (_, keys) = parse_with_top_level_keys(br#"{"x":{"a":1,"a":2},"a":3}"#)
            .expect("nested duplicates parse");
        assert_eq!(keys, vec![body_string("x"), body_string("a")]);

        let (_, keys) =
            parse_with_top_level_keys(br#"{"a":"\"b\":1,\"c\":2"}"#).expect("string text parses");
        assert_eq!(keys, vec![body_string("a")]);

        let (_, keys) = parse_with_top_level_keys(
            br#"{"\/":1,"\\":2,"\"":3,"\b":4,"\f":5,"\n":6,"\r":7,"\t":8}"#,
        )
        .expect("escaped keys parse");
        assert_eq!(
            keys,
            vec![
                body_string("/"),
                body_string("\\"),
                body_string("\""),
                body_string("\u{8}"),
                body_string("\u{c}"),
                body_string("\n"),
                body_string("\r"),
                body_string("\t"),
            ]
        );

        let (_, keys) = parse_with_top_level_keys(br#"{"\ud83e\udec0":1,"\ud800":2}"#)
            .expect("Unicode keys parse");
        assert_eq!(
            keys,
            vec![
                body_string("🫀"),
                body_string_from_code_points(vec![0xd800]),
            ]
        );
    }

    #[test]
    fn non_object_top_levels_have_no_observed_keys() {
        for input in [
            b"[1,2]".as_slice(),
            b"\"text\"",
            b"123",
            b"true",
            b"false",
            b"null",
        ] {
            let (_, keys) = parse_with_top_level_keys(input).expect("non-object parses");
            assert!(keys.is_empty());
        }
    }

    #[test]
    fn all_full_and_prefix_inputs_are_panic_free_and_agree() {
        let inputs = generated_corpus()
            .into_iter()
            .chain(fixture_inputs())
            .collect::<Vec<_>>();
        for (name, input) in inputs {
            assert_entry_points_agree(&input);
            for length in 1..input.len() {
                assert_entry_points_agree(&input[..length]);
            }
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn boundary_errors_match_exactly_across_entry_points() {
        let too_long_integer = format!("1{}", "0".repeat(4300));
        assert_exact_error(
            too_long_integer.as_bytes(),
            ParseError::NumberTooLong { byte_offset: 4300 },
        );

        let (too_deep, offset) = repeated_objects(129);
        assert_exact_error(
            &too_deep,
            ParseError::MalformedJson {
                byte_offset: offset,
            },
        );
    }

    #[test]
    fn disabled_observation_is_inert() {
        let input = br#"{"b":1,"a":2,"nested":{"c":3}}"#;
        let (value, keys) = parse_core(input, false).expect("object parses");
        assert!(keys.is_empty());
        assert_eq!(keys.capacity(), 0);
        assert_body_value_bitwise_eq(&value, &parse(input).expect("public parse succeeds"));
    }
}
