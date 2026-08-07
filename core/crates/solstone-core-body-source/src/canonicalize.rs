// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write;

use crate::parser::MAX_NESTING;
use crate::{BodyString, BodyValue, CanonicalizeError};

/// Encodes a body value as Python-compatible compact ASCII JSON.
pub fn canonicalize(value: &BodyValue) -> Result<String, CanonicalizeError> {
    let mut output = String::new();
    canonicalize_value(value, 0, &mut output)?;
    Ok(output)
}

fn canonicalize_value(
    value: &BodyValue,
    depth: usize,
    output: &mut String,
) -> Result<(), CanonicalizeError> {
    match value {
        BodyValue::Null => output.push_str("null"),
        BodyValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        BodyValue::Integer(value) => {
            if value.is_negative() {
                output.push('-');
            }
            output.push_str(value.digits());
        }
        BodyValue::Number(value) => output.push_str(&format_python_float(*value)),
        BodyValue::String(value) => write_quoted_ascii_string(value, output),
        BodyValue::Array(values) => {
            let child_depth = enter_container(depth)?;
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                canonicalize_value(value, child_depth, output)?;
            }
            output.push(']');
        }
        BodyValue::Object(values) => {
            let child_depth = enter_container(depth)?;
            output.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_quoted_ascii_string(key, output);
                output.push(':');
                canonicalize_value(value, child_depth, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
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

fn write_quoted_ascii_string(value: &BodyString, output: &mut String) {
    output.push('"');
    for code_point in value.code_points() {
        match *code_point {
            0x22 => output.push_str("\\\""),
            0x5c => output.push_str("\\\\"),
            0x08 => output.push_str("\\b"),
            0x0c => output.push_str("\\f"),
            0x0a => output.push_str("\\n"),
            0x0d => output.push_str("\\r"),
            0x09 => output.push_str("\\t"),
            0x00..=0x1f | 0x7f..=0xffff => write_unicode_escape(*code_point, output),
            0x20..=0x7e => output.push(*code_point as u8 as char),
            0x10000..=0x10ffff => {
                let reduced = *code_point - 0x10000;
                write_unicode_escape(0xd800 + (reduced >> 10), output);
                write_unicode_escape(0xdc00 + (reduced & 0x3ff), output);
            }
            _ => unreachable!("BodyString only stores code points through U+10FFFF"),
        }
    }
    output.push('"');
}

fn write_unicode_escape(code_point: u32, output: &mut String) {
    write!(output, "\\u{code_point:04x}").expect("writing to String cannot fail");
}
