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
    /// Every owner-media file is a handler-written empty audio terminal.
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

/// Split owner-media files into handler-empty-terminal vs everything else.
///
/// Exclusive and exhaustive over `found`. Callers evaluate each side
/// independently; [`classify`] still answers only whether one given slice is
/// homogeneously empty-terminal.
pub(crate) fn partition_empty_audio(
    found: Vec<FoundContent>,
    registry: &dyn HandlerRegistry,
) -> (Vec<FoundContent>, Vec<FoundContent>) {
    found
        .into_iter()
        .partition(|item| is_handler_empty_audio(item, registry))
}

pub(crate) fn is_handler_empty_audio(item: &FoundContent, registry: &dyn HandlerRegistry) -> bool {
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
        && matches!(
            record.get("reason_code").and_then(|value| value.as_str()),
            Some(
                vocab::REASON_NO_DECODABLE_AUDIO
                    | vocab::REASON_NO_SPEECH
                    | vocab::REASON_NO_TRANSCRIPT
            )
        )
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

    fn file(name: &str, record: Option<serde_json::Value>) -> FoundContent {
        FoundContent {
            name: ContentName::new(name).unwrap(),
            size: 4,
            sidecar: SidecarFacts {
                record,
                has_analysis_row: false,
            },
        }
    }

    fn audio(record: Option<serde_json::Value>) -> FoundContent {
        file("audio.flac", record)
    }

    fn names(found: &[FoundContent]) -> Vec<&str> {
        found.iter().map(|item| item.name.as_str()).collect()
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
    fn decoded_empty_audio_keeps_its_existing_retention_policy() {
        for reason in [vocab::REASON_NO_SPEECH, vocab::REASON_NO_TRANSCRIPT] {
            let mut record = empty_record();
            record["reason_code"] = json!(reason);
            assert_eq!(
                classify(&[audio(Some(record.clone()))], &ClosedHandlerSet),
                MediaClass::NoDecodableAudio
            );
            record["source"] = json!("backfill");
            assert_eq!(
                classify(&[audio(Some(record))], &ClosedHandlerSet),
                MediaClass::Ordinary
            );
        }
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

    #[test]
    fn partition_empty_audio_splits_a_mixed_list() {
        let (empty, ordinary) = partition_empty_audio(
            vec![
                file("audio.flac", Some(empty_record())),
                file(
                    "extra.flac",
                    Some(json!({
                        "schema": vocab::SCHEMA,
                        "state": vocab::STATE_ANALYZED,
                        "reason_code": vocab::REASON_OK,
                        "handler": vocab::HANDLER_TRANSCRIBE,
                    })),
                ),
            ],
            &ClosedHandlerSet,
        );
        assert_eq!(names(&empty), ["audio.flac"]);
        assert_eq!(names(&ordinary), ["extra.flac"]);
        assert_eq!(
            classify(&empty, &ClosedHandlerSet),
            MediaClass::NoDecodableAudio
        );
        assert_eq!(classify(&ordinary, &ClosedHandlerSet), MediaClass::Ordinary);
    }

    #[test]
    fn partition_empty_audio_puts_lookalikes_in_the_ordinary_side() {
        let mut backfill = empty_record();
        backfill
            .as_object_mut()
            .unwrap()
            .insert("source".to_owned(), json!("backfill"));
        let cases = [
            ("backfill", file("audio.flac", Some(backfill))),
            (
                "wrong-reason",
                file(
                    "audio.flac",
                    Some(json!({
                        "schema": vocab::SCHEMA,
                        "state": vocab::STATE_EMPTY,
                        "reason_code": vocab::REASON_OK,
                        "handler": vocab::HANDLER_TRANSCRIBE,
                    })),
                ),
            ),
            (
                "failed",
                file(
                    "audio.flac",
                    Some(json!({
                        "schema": vocab::SCHEMA,
                        "state": vocab::STATE_FAILED,
                        "reason_code": vocab::REASON_CORRUPT_INPUT,
                        "handler": vocab::HANDLER_TRANSCRIBE,
                    })),
                ),
            ),
        ];
        for (name, item) in cases {
            let (empty, ordinary) = partition_empty_audio(vec![item], &ClosedHandlerSet);
            assert!(empty.is_empty(), "{name} must not be independently empty");
            assert_eq!(names(&ordinary), ["audio.flac"], "{name}");
            assert_eq!(
                classify(&ordinary, &ClosedHandlerSet),
                MediaClass::Ordinary,
                "{name}"
            );
        }
    }

    #[test]
    fn partition_empty_audio_empty_input_and_homogeneous_lists() {
        let (empty, ordinary) = partition_empty_audio(Vec::new(), &ClosedHandlerSet);
        assert!(empty.is_empty());
        assert!(ordinary.is_empty());

        let (empty, ordinary) =
            partition_empty_audio(vec![audio(Some(empty_record()))], &ClosedHandlerSet);
        assert_eq!(names(&empty), ["audio.flac"]);
        assert!(ordinary.is_empty());

        let (empty, ordinary) = partition_empty_audio(
            vec![file(
                "extra.flac",
                Some(json!({
                    "schema": vocab::SCHEMA,
                    "state": vocab::STATE_ANALYZED,
                    "reason_code": vocab::REASON_OK,
                    "handler": vocab::HANDLER_TRANSCRIBE,
                })),
            )],
            &ClosedHandlerSet,
        );
        assert!(empty.is_empty());
        assert_eq!(names(&ordinary), ["extra.flac"]);
    }
}
