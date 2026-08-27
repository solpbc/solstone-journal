// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Internal errors for read-only profile construction.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileError {
    detail: String,
}

pub(crate) type ProfileResult<T> = Result<T, ProfileError>;

impl ProfileError {
    pub(crate) fn internal(error: impl fmt::Display) -> Self {
        Self {
            detail: error.to_string(),
        }
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for ProfileError {}
