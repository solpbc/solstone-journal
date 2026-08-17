// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which handler is expected to consume a media file, and what its analysis rows
//! look like.
//!
//! This lives beside the record vocabulary because it *is* record vocabulary: the
//! handler names it produces are the same closed set [`vocab`](crate::vocab)
//! declares, and a record only proves anything once you know which handler was
//! supposed to write it. Two copies of this map with nothing binding them is the
//! drift class one-contract-per-boundary exists to prevent — and there were two
//! until this module, one of them private inside an ingest crate.
//!
//! ⚠ This table used to be checked against `solstone/think/media.py`'s `FORMATS`
//! by a parity gate. Both the Python authority and the gate went with the
//! reference cut, so this module is now the authority rather than a copy of one.
//!
//! # No handler claims a still image, and that is load-bearing
//!
//! Images are media, and nothing in the closed set consumes them. So an image's
//! raw is never provable and a segment holding one is held — which is *correct*
//! (there is no derived output that could stand in for the original) and is why the
//! set is closed rather than defaulted.

use crate::vocab;

/// What kind of media an extension names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Audio,
    Video,
    Image,
}

/// Every media extension the journal recognises, mirroring `FORMATS`.
///
/// ⛔ Extensions are lowercase and carry **no leading dot**, unlike the Python
/// table. Callers pass what an extension accessor returns, and a dot-carrying key
/// here would silently match nothing.
const FORMATS: &[(&str, MediaKind)] = &[
    ("flac", MediaKind::Audio),
    ("opus", MediaKind::Audio),
    ("ogg", MediaKind::Audio),
    ("m4a", MediaKind::Audio),
    ("mp3", MediaKind::Audio),
    ("wav", MediaKind::Audio),
    ("webm", MediaKind::Video),
    ("mp4", MediaKind::Video),
    ("mov", MediaKind::Video),
    ("png", MediaKind::Image),
    ("jpg", MediaKind::Image),
    ("jpeg", MediaKind::Image),
    ("heic", MediaKind::Image),
    ("heif", MediaKind::Image),
    ("gif", MediaKind::Image),
    ("webp", MediaKind::Image),
    ("tiff", MediaKind::Image),
];

/// The kind of media an extension names, if the journal recognises it.
///
/// `extension` is matched case-insensitively and must not carry a leading dot.
pub fn media_kind(extension: &str) -> Option<MediaKind> {
    let lowered = extension.to_ascii_lowercase();
    FORMATS
        .iter()
        .find(|(candidate, _)| *candidate == lowered)
        .map(|(_, kind)| *kind)
}

/// Whether an extension names media at all.
pub fn is_media_extension(extension: &str) -> bool {
    media_kind(extension).is_some()
}

/// The handler expected to have consumed media with this extension.
///
/// `None` means no handler in the closed set claims it, so **no proof of
/// consumption is obtainable for it at any point.** ⛔ Never default this to a
/// handler: a wrong expected handler makes a record for some *other* file's work
/// look like proof for this one.
pub fn expected_handler(extension: &str) -> Option<&'static str> {
    match media_kind(extension)? {
        MediaKind::Audio => Some(vocab::HANDLER_TRANSCRIBE),
        MediaKind::Video => Some(vocab::HANDLER_DESCRIBE),
        // Deliberately unclaimed. See the module note.
        MediaKind::Image => None,
    }
}

/// The JSONL row key that proves a handler's analysis rows exist.
pub fn analysis_row_key(handler: &str) -> Option<&'static str> {
    match handler {
        vocab::HANDLER_TRANSCRIBE => Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY),
        vocab::HANDLER_DESCRIBE => Some(vocab::SCREEN_ANALYSIS_ROW_KEY),
        _ => None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_and_video_kinds_route_to_their_handlers() {
        assert_eq!(expected_handler("flac"), Some(vocab::HANDLER_TRANSCRIBE));
        assert_eq!(expected_handler("mp4"), Some(vocab::HANDLER_DESCRIBE));
    }

    /// ⛔ The property the retention predicate rests on.
    #[test]
    fn no_handler_claims_a_still_image_although_it_is_media() {
        for extension in ["png", "jpg", "jpeg", "heic", "heif", "gif", "webp", "tiff"] {
            assert!(is_media_extension(extension), "{extension} is media");
            assert_eq!(
                expected_handler(extension),
                None,
                "{extension} must remain unclaimed, or its raw becomes releasable \
                 with no derived output standing in for it"
            );
        }
    }

    #[test]
    fn an_unknown_extension_is_neither_media_nor_claimed() {
        for extension in ["jsonl", "txt", "pdf", "", "flacc"] {
            assert!(!is_media_extension(extension), "{extension:?}");
            assert_eq!(expected_handler(extension), None, "{extension:?}");
        }
    }

    /// A dot-carrying key is the mistake the table's shape invites.
    #[test]
    fn an_extension_with_a_leading_dot_matches_nothing() {
        assert_eq!(media_kind(".flac"), None);
        assert_eq!(expected_handler(".flac"), None);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(media_kind("FLAC"), Some(MediaKind::Audio));
        assert_eq!(expected_handler("MP4"), Some(vocab::HANDLER_DESCRIBE));
    }

    #[test]
    fn each_handler_has_exactly_one_analysis_row_key() {
        assert_eq!(
            analysis_row_key(vocab::HANDLER_TRANSCRIBE),
            Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY)
        );
        assert_eq!(
            analysis_row_key(vocab::HANDLER_DESCRIBE),
            Some(vocab::SCREEN_ANALYSIS_ROW_KEY)
        );
        assert_eq!(analysis_row_key("nosuch"), None);
        assert_ne!(
            vocab::AUDIO_TRANSCRIPT_ROW_KEY,
            vocab::SCREEN_ANALYSIS_ROW_KEY,
            "two handlers sharing one marker key would let one modality's rows \
             prove the other's work"
        );
    }

    /// Every handler the vocabulary declares is reachable from the map, so a new
    /// handler cannot be added to the closed set and left unroutable.
    #[test]
    fn every_declared_handler_is_produced_by_the_map() {
        let produced: Vec<&str> = FORMATS
            .iter()
            .filter_map(|(extension, _)| expected_handler(extension))
            .collect();
        for handler in [vocab::HANDLER_TRANSCRIBE, vocab::HANDLER_DESCRIBE] {
            assert!(
                produced.contains(&handler),
                "{handler} is declared but no extension routes to it"
            );
            assert!(
                analysis_row_key(handler).is_some(),
                "{handler} has no analysis row key"
            );
        }
    }
}
