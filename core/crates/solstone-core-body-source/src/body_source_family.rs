// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{BodySourcePolicyError, BodySourcePolicyField, BodyString};

/// A validated native body-source family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BodySourceFamily {
    AppleHealth,
    OuraApi,
}

impl BodySourceFamily {
    /// Builds a source family from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodySourcePolicyError> {
        match bytes {
            b"apple_health" => Ok(Self::AppleHealth),
            b"oura_api" => Ok(Self::OuraApi),
            _ => Err(invalid_format()),
        }
    }

    /// Builds a source family from a decoded body string.
    pub fn from_body_string(value: &BodyString) -> Result<Self, BodySourcePolicyError> {
        let mut bytes = Vec::with_capacity(value.code_points().len());
        for code_point in value.code_points() {
            if *code_point > 0x7f {
                return Err(invalid_format());
            }
            bytes.push(*code_point as u8);
        }
        Self::from_bytes(&bytes)
    }

    /// Returns the exact validated wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AppleHealth => "apple_health",
            Self::OuraApi => "oura_api",
        }
    }

    /// Returns this source family as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.as_str().bytes().map(u32::from).collect())
            .expect("validated BodySourceFamily contains only ASCII code points")
    }
}

impl TryFrom<&[u8]> for BodySourceFamily {
    type Error = BodySourcePolicyError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BodySourceFamily {
    type Error = BodySourcePolicyError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodySourcePolicyError {
    BodySourcePolicyError::InvalidFormat(BodySourcePolicyField::SourceFamily)
}
