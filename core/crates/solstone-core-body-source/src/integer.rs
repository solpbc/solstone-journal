// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// An arbitrary-size JSON integer with a canonical sign and digit sequence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BodyInteger {
    negative: bool,
    digits: Box<str>,
}

impl BodyInteger {
    /// Builds an integer from a normalized ASCII decimal digit sequence.
    pub fn new(negative: bool, digits: impl Into<Box<str>>) -> Option<Self> {
        let digits = digits.into();
        if digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || (digits.len() > 1 && digits.starts_with('0'))
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
