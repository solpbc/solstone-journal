// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Import stream-name derivation.

use std::fmt;

use regex::Regex;

/// A stream-name canonicalisation refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamNameError {
    Empty,
    DoubleDot,
    Invalid,
}

impl fmt::Display for StreamNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "stream name cannot be empty",
            Self::DoubleDot => "stream name cannot contain '..'",
            Self::Invalid => "stream name contains invalid characters",
        })
    }
}

impl std::error::Error for StreamNameError {}

/// Derives and canonicalises an import label from its selected source.
pub fn import_stream_name(import_source: &str) -> Result<String, StreamNameError> {
    canonicalize_stream_name("import", Some(import_source))
}

/// Matches Python's import-stream canonicalisation.
pub fn canonicalize_stream_name(
    base: &str,
    qualifier: Option<&str>,
) -> Result<String, StreamNameError> {
    let input = match qualifier {
        Some(qualifier) => format!("{base}.{qualifier}"),
        None => base.to_owned(),
    };
    // `streams.py:98-116` strips first. Rust regex `$` rejects a raw trailing
    // newline where Python accepts it, so this preserves the external result.
    let folded = Regex::new(r"[\s/\\]+")
        .expect("literal separator regex is valid")
        .replace_all(&input.trim().to_lowercase(), "-")
        .into_owned();
    if folded.is_empty() {
        return Err(StreamNameError::Empty);
    }
    if folded.contains("..") {
        return Err(StreamNameError::DoubleDot);
    }
    let valid = Regex::new(r"^[a-z0-9][a-z0-9._-]*$").expect("literal name regex is valid");
    if !valid.is_match(&folded) {
        return Err(StreamNameError::Invalid);
    }
    Ok(folded)
}
