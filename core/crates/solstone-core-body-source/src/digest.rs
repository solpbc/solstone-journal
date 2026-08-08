// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{BodyString, BodyWireIdentityError, BodyWireIdentityField};

const DIGEST_LENGTH: usize = 71;
const PREFIX: &[u8] = b"sha256:";
pub(crate) const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// A validated native body-bundle SHA-256 digest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyDigest(Box<str>);

impl BodyDigest {
    /// Builds a digest from its exact ASCII wire spelling.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BodyWireIdentityError> {
        if !is_valid_digest(bytes) {
            return Err(invalid_format());
        }
        let value = std::str::from_utf8(bytes).map_err(|_| invalid_format())?;
        Ok(Self(value.into()))
    }

    /// Builds a digest from a decoded body string.
    pub fn from_body_string(value: &BodyString) -> Result<Self, BodyWireIdentityError> {
        if value.code_points().len() != DIGEST_LENGTH {
            return Err(invalid_format());
        }
        let mut bytes = Vec::with_capacity(DIGEST_LENGTH);
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

    /// Returns this digest as a decoded body string.
    pub fn to_body_string(&self) -> BodyString {
        BodyString::from_code_points(self.0.bytes().map(u32::from).collect())
            .expect("validated BodyDigest contains only ASCII code points")
    }
}

impl TryFrom<&[u8]> for BodyDigest {
    type Error = BodyWireIdentityError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&BodyString> for BodyDigest {
    type Error = BodyWireIdentityError;

    fn try_from(value: &BodyString) -> Result<Self, Self::Error> {
        Self::from_body_string(value)
    }
}

fn invalid_format() -> BodyWireIdentityError {
    BodyWireIdentityError::InvalidFormat(BodyWireIdentityField::Digest)
}

fn is_valid_digest(bytes: &[u8]) -> bool {
    bytes.len() == DIGEST_LENGTH
        && bytes.starts_with(PREFIX)
        && bytes[PREFIX.len()..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
