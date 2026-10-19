// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::vocab::ScalarType;

pub const CARDINAL_MIN: i64 = -2147483648;
pub const CARDINAL_MAX: i64 = 2147483647;

pub const INTEGER_MIN: i64 = -9007199254740991;
pub const INTEGER_MAX: i64 = 9007199254740991;

pub fn validate_numeric_bounds(
    scalar_type: ScalarType,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<(), &'static str> {
    if let Some(val) = min {
        check_number(scalar_type, val)?;
    }
    if let Some(val) = max {
        check_number(scalar_type, val)?;
    }
    if let (Some(lo), Some(hi)) = (min, max)
        && lo > hi
    {
        return Err("numeric-range");
    }
    Ok(())
}

fn check_number(scalar_type: ScalarType, val: f64) -> Result<(), &'static str> {
    if !val.is_finite() {
        return Err("numeric-range");
    }
    if val.fract() != 0.0 {
        return Err("numeric-range");
    }
    let int_val = val as i64;
    match scalar_type {
        ScalarType::Cardinal => {
            if !(CARDINAL_MIN..=CARDINAL_MAX).contains(&int_val) {
                return Err("numeric-range");
            }
        }
        ScalarType::Integer => {
            if !(INTEGER_MIN..=INTEGER_MAX).contains(&int_val) {
                return Err("numeric-range");
            }
        }
        ScalarType::Boolean => return Err("not-a-number"),
        _ => return Err("numeric-range"),
    }
    Ok(())
}

pub fn check_scalar_value(
    scalar_type: ScalarType,
    val: &serde_json::Value,
) -> Result<f64, &'static str> {
    if val.is_boolean() {
        return Err("not-a-number");
    }
    let f = match val.as_f64() {
        Some(num) => num,
        None => return Err("not-a-number"),
    };
    check_number(scalar_type, f)?;
    Ok(f)
}
