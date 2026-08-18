// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// Filenames reserved for journal-authored segment metadata.
pub const RESERVED_SEGMENT_FILENAMES: [&str; 7] = [
    "stream.json",
    "ingest.json",
    "ingest.json.lock",
    "device.json",
    "events.jsonl",
    "tombstone.json",
    "shape.json",
];

/// Return whether `name` is a reserved sidecar name, compared case-insensitively.
///
/// The comparison ignores case because the filesystem does. APFS and NTFS fold
/// case by default and ext4 does not, so an exact comparison accepts
/// `TOMBSTONE.JSON` as client content and then resolves it to the journal's own
/// `tombstone.json` on two of the three platforms this ships to -- and not on
/// the one it is most likely to be developed on. The sidecars are
/// journal-authored by definition; a client that can place bytes at one of
/// their paths can pre-empt an attribution record, or an owner's evidence that
/// a deletion happened.
///
/// ASCII case folding is sufficient and deliberate: every reserved name is
/// ASCII, and Unicode folding would make the predicate depend on locale.
pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_SEGMENT_FILENAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
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
    fn rejects_reserved_names_in_any_case() {
        for name in RESERVED_SEGMENT_FILENAMES {
            assert!(matches!(
                ContentName::new(name),
                Err(ContentNameError::Reserved(_))
            ));
            // The filesystem folds case on APFS and NTFS, so these resolve to
            // the same path as the journal's own sidecar. Accepting them lets a
            // client pre-empt an attribution record or an owner's evidence of a
            // deletion, on two of the three platforms this ships to.
            assert!(
                matches!(
                    ContentName::new(&name.to_uppercase()),
                    Err(ContentNameError::Reserved(_))
                ),
                "{name} accepted in upper case"
            );
            let mixed: String = name
                .chars()
                .enumerate()
                .map(|(index, character)| {
                    if index % 2 == 0 {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    }
                })
                .collect();
            assert!(
                matches!(ContentName::new(&mixed), Err(ContentNameError::Reserved(_))),
                "{mixed} accepted in mixed case"
            );
        }
        // A name that merely resembles one is still ordinary content.
        assert!(ContentName::new("events.jsonl.bak").is_ok());
        assert!(ContentName::new("my-tombstone.json").is_ok());
    }

    #[test]
    fn shape_sidecar_is_a_reserved_content_name() {
        assert!(RESERVED_SEGMENT_FILENAMES.contains(&"shape.json"));
        assert!(matches!(
            ContentName::new("shape.json"),
            Err(ContentNameError::Reserved(_))
        ));
        assert!(matches!(
            ContentName::new("SHAPE.JSON"),
            Err(ContentNameError::Reserved(_))
        ));
        assert!(matches!(
            ContentName::new("Shape.Json"),
            Err(ContentNameError::Reserved(_))
        ));
        assert!(ContentName::new("my-shape.json").is_ok());
        assert!(ContentName::new("shape.json.bak").is_ok());
    }

    #[test]
    fn rejects_non_plain_names() {
        for name in ["", ".", "..", "nested/file", "nested\\file"] {
            assert!(ContentName::new(name).is_err(), "{name:?}");
        }
    }
}
