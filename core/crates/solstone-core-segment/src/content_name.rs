// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// Filenames reserved for journal-authored segment metadata.
pub const RESERVED_SEGMENT_FILENAMES: [&str; 5] = [
    "stream.json",
    "ingest.json",
    "ingest.json.lock",
    "device.json",
    "events.jsonl",
];

/// Return whether `name` is one of the exact, case-sensitive reserved names.
pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_SEGMENT_FILENAMES.contains(&name)
}

/// A validated, non-reserved single filename for client content bytes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContentName(String);

impl ContentName {
    /// Construct a content filename that cannot name a journal sidecar.
    pub fn new(name: &str) -> Result<Self, ContentNameError> {
        if name.is_empty() {
            return Err(ContentNameError::Empty);
        }
        if name.contains('/') || name.contains('\\') {
            return Err(ContentNameError::NotPlain(name.to_owned()));
        }
        if matches!(name, "." | "..") {
            return Err(ContentNameError::NotPlain(name.to_owned()));
        }
        if is_reserved_name(name) {
            return Err(ContentNameError::Reserved(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    /// The validated filename.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ContentName {
    type Error = ContentNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for ContentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Failure to construct a safe content filename.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentNameError {
    Empty,
    NotPlain(String),
    Reserved(String),
}

impl fmt::Display for ContentNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("content name must not be empty"),
            Self::NotPlain(name) => write!(formatter, "content name is not plain: {name:?}"),
            Self::Reserved(name) => write!(formatter, "content name is reserved: {name}"),
        }
    }
}

impl std::error::Error for ContentNameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_exact_reserved_names_without_case_folding() {
        for name in RESERVED_SEGMENT_FILENAMES {
            assert!(matches!(
                ContentName::new(name),
                Err(ContentNameError::Reserved(_))
            ));
        }
        assert!(ContentName::new("EVENTS.JSONL").is_ok());
    }

    #[test]
    fn rejects_non_plain_names() {
        for name in ["", ".", "..", "nested/file", "nested\\file"] {
            assert!(ContentName::new(name).is_err(), "{name:?}");
        }
    }
}
