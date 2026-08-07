// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// A token-free body-source parse failure with a raw UTF-8 byte offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    MalformedJson { byte_offset: usize },
    NumberTooLong { byte_offset: usize },
}

impl ParseError {
    pub(crate) const fn malformed(byte_offset: usize) -> Self {
        Self::MalformedJson { byte_offset }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedJson { byte_offset } => {
                write!(formatter, "malformed_json at byte offset {byte_offset}")
            }
            Self::NumberTooLong { byte_offset } => {
                write!(formatter, "number_too_long at byte offset {byte_offset}")
            }
        }
    }
}

impl std::error::Error for ParseError {}
