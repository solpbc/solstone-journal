// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{BodyString, BodyWireIdentityError, BodyWireIdentityField};

const BUNDLE_ID_LENGTH: usize = 31;
const PREFIX: &[u8] = b"body-";
const CROCKFORD32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A validated native body-bundle identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BundleId(Box<str>);

impl BundleId {
    /// Builds a bundle identifier from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodyWireIdentityError> {
        if !is_valid_bundle_id(bytes) {
            return Err(invalid_format());
        }
        let value = std::str::from_utf8(bytes).map_err(|_| invalid_format())?;
        Ok(Self(value.into()))
    }

    /// Builds a bundle identifier from a decoded body string.
    pub fn from_body_string(value: &BodyString) -> Result<Self, BodyWireIdentityError> {
        if value.code_points().len() != BUNDLE_ID_LENGTH {
            return Err(invalid_format());
        }
        let mut bytes = Vec::with_capacity(BUNDLE_ID_LENGTH);
        for code_point in value.code_points() {
            if *code_point > 0x7f {
                return Err(invalid_format());
            }
            bytes.push(*code_point as u8);
        }
        Self::from_bytes(&bytes)
    }

    /// Returns the exact validated wire spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns this identifier as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.0.bytes().map(u32::from).collect())
            .expect("validated BundleId contains only ASCII code points")
    }
}

impl TryFrom<&[u8]> for BundleId {
    type Error = BodyWireIdentityError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BundleId {
    type Error = BodyWireIdentityError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodyWireIdentityError {
    BodyWireIdentityError::InvalidFormat(BodyWireIdentityField::BundleId)
}

fn is_valid_bundle_id(bytes: &[u8]) -> bool {
    bytes.len() == BUNDLE_ID_LENGTH
        && bytes.starts_with(PREFIX)
        && matches!(bytes.get(PREFIX.len()), Some(b'0'..=b'7'))
        && bytes[PREFIX.len() + 1..]
            .iter()
            .all(|byte| CROCKFORD32.contains(byte))
}
