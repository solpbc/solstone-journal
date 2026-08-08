// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// A Python-compatible JSON integer of up to 4,300 decimal digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodyInteger {
    negative: bool,
    digits: Box<str>,
}

impl BodyInteger {
    /// Builds a non-negative integer from a `u64` value.
    pub fn from_u64(value: u64) -> Self {
        Self::new(false, value.to_string()).expect("u64 decimal digits are always valid")
    }

    /// Builds an integer from a normalized ASCII decimal digit sequence.
    pub fn new(negative: bool, digits: impl Into<Box<str>>) -> Option<Self> {
        let digits = digits.into();
        if digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || (digits.len() > 1 && digits.starts_with('0'))
            || digits.len() > 4300
        {
            return None;
        }
        let negative = negative && digits.as_ref() != "0";
        Some(Self { negative, digits })
    }

    /// Whether this nonzero integer is negative.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// The normalized decimal digits, without a sign.
    pub fn digits(&self) -> &str {
        &self.digits
    }
}
