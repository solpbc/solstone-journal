// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use thiserror::Error;

/// A wire request could not form a runnable command.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WireRequestError {
    #[error("task request is missing cmd")]
    MissingCommand,
}
