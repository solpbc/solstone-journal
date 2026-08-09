// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::convert::Infallible;

use crate::parser::MAX_NESTING;
use crate::{BodyValue, CanonicalizeError};

pub(crate) trait CanonicalSink {
    type Error;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

impl CanonicalSink for String {
    type Error = Infallible;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.push_str(std::str::from_utf8(bytes).expect("canonical JSON grammar is ASCII"));
        Ok(())
    }
}

pub(crate) struct CappedVecSink<'a> {
    output: &'a mut Vec<u8>,
    limit: usize,
}

impl<'a> CappedVecSink<'a> {
    pub(crate) fn new(output: &'a mut Vec<u8>, limit: usize) -> Self {
        Self { output, limit }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CappedSinkError {
    InputTooLarge,
}

impl CanonicalSink for CappedVecSink<'_> {
    type Error = CappedSinkError;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes.len() > self.limit.saturating_sub(self.output.len()) {
            return Err(CappedSinkError::InputTooLarge);
        }
        self.output.extend_from_slice(bytes);
        Ok(())
    }
}

/// Encodes a body value as Python-compatible compact ASCII JSON.
pub fn canonicalize(value: &BodyValue) -> Result<String, CanonicalizeError> {
    let mut output = String::new();
    canonicalize_value(value, 0, &mut output).map_err(|error| match error {
        CanonicalizeValueError::Canonicalize(error) => error,
        CanonicalizeValueError::Sink(error) => match error {},
    })?;
    Ok(output)
}

enum CanonicalizeValueError<E> {
    Canonicalize(CanonicalizeError),
    Sink(E),
}

fn canonicalize_value<S: CanonicalSink>(
    value: &BodyValue,
    depth: usize,
    output: &mut S,
) -> Result<(), CanonicalizeValueError<S::Error>> {
    match value {
        BodyValue::Null => output
            .write_bytes(b"null")
            .map_err(CanonicalizeValueError::Sink)?,
        BodyValue::Bool(value) => output
            .write_bytes(if *value { b"true" } else { b"false" })
            .map_err(CanonicalizeValueError::Sink)?,
        BodyValue::Integer(value) => write_integer(output, value.is_negative(), value.digits())
            .map_err(CanonicalizeValueError::Sink)?,
        BodyValue::Number(value) => output
            .write_bytes(format_python_float(*value).as_bytes())
            .map_err(CanonicalizeValueError::Sink)?,
        BodyValue::String(value) => {
            write_quoted_code_points(output, value.code_points().iter().copied())
                .map_err(CanonicalizeValueError::Sink)?;
        }
        BodyValue::Array(values) => {
            let child_depth =
                enter_container(depth).map_err(CanonicalizeValueError::Canonicalize)?;
            write_array_start(output).map_err(CanonicalizeValueError::Sink)?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    write_separator(output).map_err(CanonicalizeValueError::Sink)?;
                }
                canonicalize_value(value, child_depth, output)?;
            }
            write_array_end(output).map_err(CanonicalizeValueError::Sink)?;
        }
        BodyValue::Object(values) => {
            let child_depth =
                enter_container(depth).map_err(CanonicalizeValueError::Canonicalize)?;
            write_object_start(output).map_err(CanonicalizeValueError::Sink)?;
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    write_separator(output).map_err(CanonicalizeValueError::Sink)?;
                }
                write_object_key(output, key.code_points().iter().copied())
                    .map_err(CanonicalizeValueError::Sink)?;
                canonicalize_value(value, child_depth, output)?;
            }
            write_object_end(output).map_err(CanonicalizeValueError::Sink)?;
        }
    }
    Ok(())
}

pub(crate) fn write_integer<S: CanonicalSink>(
    output: &mut S,
    negative: bool,
    digits: &str,
) -> Result<(), S::Error> {
    if negative {
        output.write_bytes(b"-")?;
    }
    output.write_bytes(digits.as_bytes())
}

pub(crate) fn write_quoted_code_points<S: CanonicalSink, I: Iterator<Item = u32>>(
    output: &mut S,
    code_points: I,
) -> Result<(), S::Error> {
    output.write_bytes(b"\"")?;
    for code_point in code_points {
        match code_point {
            0x22 => output.write_bytes(b"\\\"")?,
            0x5c => output.write_bytes(b"\\\\")?,
            0x08 => output.write_bytes(b"\\b")?,
            0x0c => output.write_bytes(b"\\f")?,
            0x0a => output.write_bytes(b"\\n")?,
            0x0d => output.write_bytes(b"\\r")?,
            0x09 => output.write_bytes(b"\\t")?,
            0x00..=0x1f | 0x7f..=0xffff => write_unicode_escape(output, code_point)?,
            0x20..=0x7e => output.write_bytes(&[code_point as u8])?,
            0x10000..=0x10ffff => {
                let reduced = code_point - 0x10000;
                write_unicode_escape(output, 0xd800 + (reduced >> 10))?;
                write_unicode_escape(output, 0xdc00 + (reduced & 0x3ff))?;
            }
            _ => unreachable!("BodyString only stores code points through U+10FFFF"),
        }
    }
    output.write_bytes(b"\"")
}

pub(crate) fn write_object_start<S: CanonicalSink>(output: &mut S) -> Result<(), S::Error> {
    output.write_bytes(b"{")
}

pub(crate) fn write_object_end<S: CanonicalSink>(output: &mut S) -> Result<(), S::Error> {
    output.write_bytes(b"}")
}

pub(crate) fn write_array_start<S: CanonicalSink>(output: &mut S) -> Result<(), S::Error> {
    output.write_bytes(b"[")
}

pub(crate) fn write_array_end<S: CanonicalSink>(output: &mut S) -> Result<(), S::Error> {
    output.write_bytes(b"]")
}

pub(crate) fn write_separator<S: CanonicalSink>(output: &mut S) -> Result<(), S::Error> {
    output.write_bytes(b",")
}

pub(crate) fn write_object_key<S: CanonicalSink, I: Iterator<Item = u32>>(
    output: &mut S,
    key: I,
) -> Result<(), S::Error> {
    write_quoted_code_points(output, key)?;
    output.write_bytes(b":")
}

fn enter_container(depth: usize) -> Result<usize, CanonicalizeError> {
    if depth >= MAX_NESTING {
        return Err(CanonicalizeError::ValueTooDeep { depth: depth + 1 });
    }
    Ok(depth + 1)
}

fn format_python_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity".to_owned()
        } else {
            "Infinity".to_owned()
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }

    let mut buffer = ryu::Buffer::new();
    let rendered = buffer.format_finite(value);
    let (sign, unsigned) = rendered
        .strip_prefix('-')
        .map_or(("", rendered), |unsigned| ("-", unsigned));
    let (coefficient, exponent) = unsigned.split_once(['e', 'E']).map_or_else(
        || (unsigned, 0),
        |(coefficient, exponent)| {
            (
                coefficient,
                exponent.parse::<i32>().expect("ryu emits an i32 exponent"),
            )
        },
    );
    let decimal_at = coefficient.find('.').unwrap_or(coefficient.len()) as i32 + exponent;
    let digits = coefficient.replace('.', "");
    let first_nonzero = digits
        .find(|character: char| character != '0')
        .expect("nonzero finite float has a nonzero digit");
    let significant = &digits[first_nonzero..];
    let scientific_exponent = decimal_at - first_nonzero as i32 - 1;

    if !(-4..16).contains(&scientific_exponent) {
        let mantissa = if significant.len() == 1 {
            significant.to_owned()
        } else {
            format!("{}.{}", &significant[..1], &significant[1..])
        };
        return format!(
            "{sign}{mantissa}e{}{magnitude:02}",
            if scientific_exponent.is_negative() {
                '-'
            } else {
                '+'
            },
            magnitude = scientific_exponent.unsigned_abs()
        );
    }

    let decimal = decimal_at - first_nonzero as i32;
    let mut fixed = if decimal <= 0 {
        format!(
            "0.{}{}",
            "0".repeat(decimal.unsigned_abs() as usize),
            significant
        )
    } else if decimal as usize >= significant.len() {
        format!(
            "{}{}",
            significant,
            "0".repeat(decimal as usize - significant.len())
        )
    } else {
        format!(
            "{}.{}",
            &significant[..decimal as usize],
            &significant[decimal as usize..]
        )
    };
    if !fixed.contains('.') {
        fixed.push_str(".0");
    }
    format!("{sign}{fixed}")
}

fn write_unicode_escape<S: CanonicalSink>(output: &mut S, code_point: u32) -> Result<(), S::Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = [
        b'\\',
        b'u',
        HEX[((code_point >> 12) & 0xf) as usize],
        HEX[((code_point >> 8) & 0xf) as usize],
        HEX[((code_point >> 4) & 0xf) as usize],
        HEX[(code_point & 0xf) as usize],
    ];
    output.write_bytes(&bytes)
}
