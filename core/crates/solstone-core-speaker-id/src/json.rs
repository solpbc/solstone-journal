// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSON serialization compatible with Python's default `json.dumps` escaping.

use std::error::Error;
use std::fmt::{self, Write as _};

use serde::Serialize;
use serde_json::Value;
use serde_json::ser::PrettyFormatter;

/// Errors produced while serializing JSON.
#[derive(Debug)]
pub enum JsonError {
    Serialize(serde_json::Error),
    Utf8(std::string::FromUtf8Error),
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => write!(f, "could not serialize JSON: {error}"),
            Self::Utf8(error) => write!(f, "JSON serializer emitted invalid UTF-8: {error}"),
        }
    }
}

impl Error for JsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::Utf8(error) => Some(error),
        }
    }
}

/// Serialize a value using Python-compatible indentation and ASCII escaping.
///
/// This matches `json.dumps(value, indent=indent)`'s default `ensure_ascii=True`
/// behavior, but deliberately leaves the trailing newline to the caller.
pub fn write_python_compatible_json(value: &Value, indent: usize) -> Result<String, JsonError> {
    let mut pretty = Vec::new();
    let indent = vec![b' '; indent];
    let formatter = PrettyFormatter::with_indent(&indent);
    let mut serializer = serde_json::Serializer::with_formatter(&mut pretty, formatter);
    value
        .serialize(&mut serializer)
        .map_err(JsonError::Serialize)?;

    let pretty = String::from_utf8(pretty).map_err(JsonError::Utf8)?;
    Ok(escape_non_ascii(&pretty))
}

fn escape_non_ascii(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());

    for character in input.chars() {
        let codepoint = character as u32;
        if codepoint <= 0x7f {
            escaped.push(character);
        } else if codepoint <= 0xffff {
            write!(escaped, "\\u{codepoint:04x}").expect("writing to a String cannot fail");
        } else {
            let value = codepoint - 0x1_0000;
            let high = 0xd800 + (value >> 10);
            let low = 0xdc00 + (value & 0x3ff);
            write!(escaped, "\\u{high:04x}\\u{low:04x}").expect("writing to a String cannot fail");
        }
    }

    escaped
}
