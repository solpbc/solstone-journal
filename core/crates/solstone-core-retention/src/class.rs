// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segment media class for empty-terminal audio.
//!
//! The class is journal-global; `per_stream` does not name it.
//!
//! A segment is [`MediaClass::NoDecodableAudio`] only when every owner-media
//! file carries a handler-written empty-terminal record. Anything else —
//! including an empty file list and a backfill guess — is [`MediaClass::Ordinary`].

use solstone_core_processing_record::vocab;

use crate::content::HandlerRegistry;
use crate::eligibility::FoundContent;

/// Which retention rule a segment is measured against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaClass {
    /// Ordinary raw media. Uses the stream's raw-media rule and the minimum-age floor.
    Ordinary,
    /// Every owner-media file is a no-decodable-audio empty terminal.
    NoDecodableAudio,
}

/// Classify a segment from the files [`scan_segment`](crate::scan::scan_segment) found.
///
/// `registry` is the same handler table [`crate::eligibility::resolve`] uses.
/// An empty file list is never the empty-audio class.
pub fn classify(found: &[FoundContent], registry: &dyn HandlerRegistry) -> MediaClass {
    if found.is_empty() {
        return MediaClass::Ordinary;
    }
    if found
        .iter()
        .all(|item| is_handler_empty_audio(item, registry))
    {
        MediaClass::NoDecodableAudio
    } else {
        MediaClass::Ordinary
    }
}

fn is_handler_empty_audio(item: &FoundContent, registry: &dyn HandlerRegistry) -> bool {
    let Some(record) = item
        .sidecar
        .record
        .as_ref()
        .and_then(|value| value.as_object())
    else {
        return false;
    };
    let Some(expected) = registry.expected_handler(&item.name) else {
        return false;
    };
    record.get("schema").and_then(|value| value.as_str()) == Some(vocab::SCHEMA)
        && record.get("state").and_then(|value| value.as_str()) == Some(vocab::STATE_EMPTY)
        && record.get("reason_code").and_then(|value| value.as_str())
            == Some(vocab::REASON_NO_DECODABLE_AUDIO)
        && record.get("handler").and_then(|value| value.as_str()) == Some(expected)
        && record.get("source").and_then(|value| value.as_str()) != Some("backfill")
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
    use crate::content::{ClosedHandlerSet, ContentName};
    use crate::eligibility::SidecarFacts;
    use serde_json::json;

    fn audio(record: Option<serde_json::Value>) -> FoundContent {
        FoundContent {
            name: ContentName::new("audio.flac").unwrap(),
            size: 4,
            sidecar: SidecarFacts {
                record,
                has_analysis_row: false,
            },
        }
    }

    fn empty_record() -> serde_json::Value {
        json!({
            "schema": vocab::SCHEMA,
            "state": vocab::STATE_EMPTY,
            "reason_code": vocab::REASON_NO_DECODABLE_AUDIO,
            "handler": vocab::HANDLER_TRANSCRIBE,
        })
    }

    #[test]
    fn an_empty_file_list_is_ordinary() {
        assert_eq!(classify(&[], &ClosedHandlerSet), MediaClass::Ordinary);
    }

    #[test]
    fn a_handler_empty_audio_file_is_the_class() {
        assert_eq!(
            classify(&[audio(Some(empty_record()))], &ClosedHandlerSet),
            MediaClass::NoDecodableAudio
        );
    }

    #[test]
    fn a_backfill_guess_is_ordinary() {
        let mut record = empty_record();
        record
            .as_object_mut()
            .unwrap()
            .insert("source".to_owned(), json!("backfill"));
        assert_eq!(
            classify(&[audio(Some(record))], &ClosedHandlerSet),
            MediaClass::Ordinary
        );
    }
}
