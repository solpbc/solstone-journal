// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Two questions about a content file that this crate refuses to answer itself.
//!
//! *Which handler is expected to have consumed this?* and *is this owner media at
//! all?* are both answered by an extension map, and that map belongs to the
//! media-processing boundary, which owns the handler registry. A second copy here
//! would be two places owning one thing with nothing binding them — the drift
//! class the one-contract-per-boundary rule exists to make unrepresentable.
//!
//! The production implementors are at the bottom of this file, and both are thin
//! delegations to the shared map in the record-vocabulary crate — which is the point:
//! when the still-image handler joins the closed set, image raw becomes releasable
//! with **no change to this crate**, because the closed set is not stated here.
//!
//! ⚠ The traits remain, rather than the map being called directly, because the
//! decision functions must stay testable against a *stated* closed set. A test that
//! passes the production registry cannot tell you which condition held.

/// A validated single filename for content inside a segment.
///
/// Thin on purpose: the segment boundary already owns the authoritative version
/// with its reserved-name rules, and this crate does not depend on that boundary
/// yet. ⛔ What matters here is that it cannot be a path, so a caller cannot reach
/// out of the segment through a content name.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContentName(String);

impl ContentName {
    /// Accept a plain filename. Rejects anything that could traverse.
    pub fn new(name: &str) -> Option<Self> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name == "."
            || name == ".."
        {
            return None;
        }
        Some(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// The lowercase extension, if any, for a classifier's use.
    pub fn extension(&self) -> Option<String> {
        let (stem, extension) = self.0.rsplit_once('.')?;
        // A leading-dot name has no extension: `.flac` is a hidden file called
        // `.flac`, not a FLAC. ⚠ The two reference implementations disagree here,
        // in the direction that releases owner data, and the narrower reading is
        // the safe one.
        if stem.is_empty() {
            return None;
        }
        Some(extension.to_ascii_lowercase())
    }
}

/// Which handler is expected to have consumed a content file.
///
/// `None` means no handler in the closed set claims it, so no proof is obtainable
/// for it at any point — which is what holds a still image.
pub trait HandlerRegistry {
    fn expected_handler(&self, name: &ContentName) -> Option<&str>;
}

impl<F> HandlerRegistry for F
where
    F: Fn(&ContentName) -> Option<&'static str>,
{
    fn expected_handler(&self, name: &ContentName) -> Option<&str> {
        self(name)
    }
}

/// Whether a name is the owner's media at all, as opposed to a sidecar, a derived
/// output or journal-authored metadata.
pub trait MediaClassifier {
    fn is_owner_media(&self, name: &ContentName) -> bool;
}

impl<F> MediaClassifier for F
where
    F: Fn(&ContentName) -> bool,
{
    fn is_owner_media(&self, name: &ContentName) -> bool {
        self(name)
    }
}

/// The production handler registry: the shared closed set.
///
/// ⛔ Stateless and unconfigurable on purpose. A registry a caller could extend
/// would let a caller grant proof to content no handler consumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClosedHandlerSet;

impl HandlerRegistry for ClosedHandlerSet {
    fn expected_handler(&self, name: &ContentName) -> Option<&str> {
        solstone_core_processing_record::expected_handler(&name.extension()?)
    }
}

/// The production media classifier: the shared format table.
///
/// ⚠ Deliberately **broad**. Being wrong in the inclusive direction holds a segment
/// (an unclaimed media file blocks it); being wrong in the exclusive direction lets
/// a file skip the predicate entirely. Only the first is safe, so anything the
/// journal calls media is classified as the owner's.
#[derive(Clone, Copy, Debug, Default)]
pub struct JournalMedia;

impl MediaClassifier for JournalMedia {
    fn is_owner_media(&self, name: &ContentName) -> bool {
        name.extension().is_some_and(|extension| {
            solstone_core_processing_record::is_media_extension(&extension)
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use super::*;

    #[test]
    fn a_content_name_cannot_traverse() {
        for name in ["", ".", "..", "a/b", "a\\b", "../escape.flac"] {
            assert!(ContentName::new(name).is_none(), "{name:?}");
        }
        assert!(ContentName::new("audio.flac").is_some());
    }

    #[test]
    fn a_leading_dot_name_has_no_extension() {
        // The narrower of the two reference readings: `.flac` is a hidden file,
        // not audio. The looser one grants proof for a client-supplied name and
        // releases the owner's data.
        assert_eq!(ContentName::new(".flac").unwrap().extension(), None);
        assert_eq!(ContentName::new(".mp4").unwrap().extension(), None);
        assert_eq!(
            ContentName::new("audio.flac")
                .unwrap()
                .extension()
                .as_deref(),
            Some("flac")
        );
        assert_eq!(
            ContentName::new(".hidden.flac")
                .unwrap()
                .extension()
                .as_deref(),
            Some("flac")
        );
        assert_eq!(
            ContentName::new("audio.FLAC")
                .unwrap()
                .extension()
                .as_deref(),
            Some("flac")
        );
    }

    /// The production pair, and the asymmetry that makes an image held.
    #[test]
    fn an_image_is_owner_media_that_no_handler_claims() {
        let image = ContentName::new("photo.png").unwrap();
        assert!(JournalMedia.is_owner_media(&image));
        assert_eq!(
            ClosedHandlerSet.expected_handler(&image),
            None,
            "so the predicate holds the segment rather than releasing the original"
        );

        let audio = ContentName::new("audio.flac").unwrap();
        assert!(JournalMedia.is_owner_media(&audio));
        assert_eq!(
            ClosedHandlerSet.expected_handler(&audio),
            Some("transcribe")
        );
    }

    /// A derived output is not the owner's media, so it is never a candidate.
    #[test]
    fn derived_and_metadata_files_are_not_owner_media() {
        for name in ["audio.jsonl", "tombstone.json", "notes.txt", "manifest"] {
            let name = ContentName::new(name).unwrap();
            assert!(!JournalMedia.is_owner_media(&name), "{}", name.as_str());
            assert_eq!(ClosedHandlerSet.expected_handler(&name), None);
        }
    }

    /// ⛔ The narrow reading carries through to the production pair.
    #[test]
    fn a_leading_dot_name_is_not_owner_media_and_is_unclaimed() {
        let hidden = ContentName::new(".flac").unwrap();
        assert!(!JournalMedia.is_owner_media(&hidden));
        assert_eq!(ClosedHandlerSet.expected_handler(&hidden), None);
    }
}
