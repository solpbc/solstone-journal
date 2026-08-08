// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{BodySourceFamily, BodySourcePolicyError, BodySourcePolicyField, BodyString};

/// A validated native body-source raw-retention policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BodyRawRetention {
    Discard,
    RetainComplete,
    RetainParsed,
}

impl BodyRawRetention {
    /// Builds a raw-retention policy from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodySourcePolicyError> {
        match bytes {
            b"discard" => Ok(Self::Discard),
            b"retain_complete" => Ok(Self::RetainComplete),
            b"retain_parsed" => Ok(Self::RetainParsed),
            _ => Err(invalid_format()),
        }
    }

    /// Builds a raw-retention policy from a decoded body string.
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
            Self::Discard => "discard",
            Self::RetainComplete => "retain_complete",
            Self::RetainParsed => "retain_parsed",
        }
    }

    /// Returns this raw-retention policy as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.as_str().bytes().map(u32::from).collect())
            .expect("validated BodyRawRetention contains only ASCII code points")
    }

    /// Rejects raw-retention policies that are incompatible with a source family.
    pub fn check_compatible(&self, family: &BodySourceFamily) -> Result<(), BodySourcePolicyError> {
        if matches!(self, Self::RetainComplete) && matches!(family, BodySourceFamily::OuraApi) {
            Err(BodySourcePolicyError::Incompatible(
                BodySourcePolicyField::RawRetention,
            ))
        } else {
            Ok(())
        }
    }
}

impl TryFrom<&[u8]> for BodyRawRetention {
    type Error = BodySourcePolicyError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BodyRawRetention {
    type Error = BodySourcePolicyError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodySourcePolicyError {
    BodySourcePolicyError::InvalidFormat(BodySourcePolicyField::RawRetention)
}
