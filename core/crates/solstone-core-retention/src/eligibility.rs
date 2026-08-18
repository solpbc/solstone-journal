// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether a segment's raw originals may be released.
//!
//! Retention does not have a predicate of its own. It consumes the
//! media-processing boundary's terminal-proof conjunction, per content file, and
//! holds the whole segment if any file blocks — because the derived output of one
//! file can depend on another's raw, and a segment is the unit an owner reasons
//! about.
//!
//! # Why this is not the same predicate as the ingest path's
//!
//! It is the same *conjunction*, called with a different and correct input, plus
//! one condition the conjunction does not contain.
//!
//! The ingest path asks *"was the file the device uploaded consumed, so the device
//! may drop its copy?"* and supplies the size the device declared. This asks
//! *"may I delete the last copy?"* and supplies **the size on disk**, because
//! these are the bytes about to go: a file replaced or truncated after processing
//! then fails and is held.
//!
//! And a record claiming `analyzed` whose analysis rows are gone satisfies the
//! conjunction while failing this question. The ingest path is right to accept it
//! — the journal did receive the upload. But if the rows are gone the raw is the
//! only surviving copy of that content, so releasing it is unrecoverable. That is
//! [`Condition::AnalyzedRowsPresent`], and the committed release oracle fails by
//! name without it.

use serde::Serialize;
use solstone_core_processing_record::{TerminalProofOutcome, evaluate_terminal_proof, vocab};

use crate::content::{ContentName, HandlerRegistry, MediaClassifier};

/// How a release was justified.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// A processing record satisfied every condition.
    Record,
    /// Pre-record data: the sidecar carries analysis rows but no processing
    /// record.
    ///
    /// Honoured because the rows genuinely are evidence the media was consumed,
    /// and the read-old rule requires older journal data to stay usable — but it
    /// is weaker evidence than a record, so a receipt must be able to say which
    /// files rested on it. ⚠ A future pass can retire this arm once the count
    /// reaches zero on real journals.
    LegacyRows,
}

/// Why one content file cannot be released.
///
/// ⛔ The variants are distinct because they are different things to tell an
/// owner. A single "not eligible" would compute the distinction and discard it,
/// and the storage-warning copy is the only place an owner ever learns why their
/// disk stopped shrinking.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "blocker")]
pub enum Blocker {
    /// Processing has not finished, or its evidence does not hold.
    Incomplete { name: String, because: &'static str },
    /// Processing failed. ⛔ Never released; disclosed.
    Failed { name: String, state: String },
    /// No handler in the closed set claims this content, so no proof is
    /// obtainable for it at any point.
    ///
    /// ⚠ This is what keeps a still image from being released: the handler set
    /// does not cover images, so nothing can ever prove one was consumed. It
    /// resolves itself when the image handler joins the registry — with no change
    /// to this crate, which is the whole reason the registry is injected.
    Unprovable { name: String },
}

impl Blocker {
    pub fn name(&self) -> &str {
        match self {
            Self::Incomplete { name, .. }
            | Self::Failed { name, .. }
            | Self::Unprovable { name } => name,
        }
    }
}

/// Raw content whose processing is proven terminal, bound to where it was proven.
///
/// 🔴 The segment is part of the value, not a sibling parameter. Without it,
/// nothing binds a proof obtained for one segment to the segment it is later
/// passed with, so a caller could hand segment A's proven `audio.flac` alongside
/// segment B and delete B's `audio.flac`, which was never proven. The guardrail
/// is only structural if the proof carries its own location.
///
/// ⛔ Constructible only in this module, and only by [`resolve`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvenRaw {
    day: String,
    stream: String,
    /// The segment's directory name. ⛔ Never a key derived from it.
    dir: String,
    name: String,
    size: u64,
    evidence: Evidence,
}

impl ProvenRaw {
    pub fn day(&self) -> &str {
        &self.day
    }
    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn dir(&self) -> &str {
        &self.dir
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn evidence(&self) -> Evidence {
        self.evidence
    }

    /// This content's segment directory, relative to the journal root.
    pub fn segment_rel(&self) -> String {
        crate::layout::segment_rel(&self.day, &self.stream, &self.dir)
    }

    /// The path of this content, relative to the journal root.
    pub fn rel(&self) -> String {
        crate::layout::content_rel(&self.day, &self.stream, &self.dir, &self.name)
    }

    /// Build a proven value in tests, running the same validation `resolve` does.
    ///
    /// ⚠ Test-only, and it validates rather than rubber-stamping: it is the only
    /// oracle the wave that lands the release verb has for the claim that a caller
    /// cannot name a non-media file, so a permissive version would make that
    /// wave's tests prove the opposite of the guarantee.
    #[cfg(test)]
    pub(crate) fn for_test(
        classifier: &dyn MediaClassifier,
        day: &str,
        stream: &str,
        dir: &str,
        name: &str,
        size: u64,
    ) -> Option<Self> {
        let content = ContentName::new(name)?;
        if !classifier.is_owner_media(&content) {
            return None;
        }
        Some(Self {
            day: day.to_owned(),
            stream: stream.to_owned(),
            dir: dir.to_owned(),
            name: content.into_string(),
            size,
            evidence: Evidence::Record,
        })
    }
}

/// One content file's verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileVerdict {
    Releasable(ProvenRaw),
    Held(Blocker),
}

/// The five conditions, in the order they are decided.
///
/// Named so a test can remove one and see which rows flip, and so a reader can
/// tell which are the borrowed conjunction and which are this caller's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Condition {
    /// A handler in the closed set claims this content. ⛔ This caller's.
    HandlerClaimsContent,
    /// The record does not report failure. ⛔ This caller's: the borrowed
    /// conjunction reports a failed state, a wrong handler and a size mismatch
    /// all as one outcome, so a failed record could never be disclosed as failed.
    NotFailed,
    /// The borrowed conjunction held: schema, terminal state, handler, size.
    TerminalProof,
    /// A record claiming `analyzed` has its analysis rows on disk.
    /// ⛔ This caller's, and the one the conjunction does not contain.
    AnalyzedRowsPresent,
}

/// What a sidecar carries, as far as this predicate needs to know.
///
/// Reading the sidecar is the caller's job; this module decides, so that the
/// decision is testable without a filesystem.
#[derive(Clone, Debug, Default)]
pub struct SidecarFacts {
    /// The `_solstone_processing` record, if the header carried one.
    pub record: Option<serde_json::Value>,
    /// Whether any early row carries the modality's analysis marker key.
    pub has_analysis_row: bool,
}

/// Decide one content file.
pub fn resolve_file(
    registry: &dyn HandlerRegistry,
    day: &str,
    stream: &str,
    dir: &str,
    name: &ContentName,
    size_on_disk: u64,
    sidecar: &SidecarFacts,
) -> FileVerdict {
    let proven = |evidence| {
        FileVerdict::Releasable(ProvenRaw {
            day: day.to_owned(),
            stream: stream.to_owned(),
            dir: dir.to_owned(),
            name: name.as_str().to_owned(),
            size: size_on_disk,
            evidence,
        })
    };
    let held = |because| {
        FileVerdict::Held(Blocker::Incomplete {
            name: name.as_str().to_owned(),
            because,
        })
    };

    // Condition::HandlerClaimsContent
    let Some(expected_handler) = registry.expected_handler(name) else {
        return FileVerdict::Held(Blocker::Unprovable {
            name: name.as_str().to_owned(),
        });
    };

    let Some(record) = sidecar.record.as_ref() else {
        // Read old. Analysis rows are real evidence the media was consumed, and
        // pre-record journals have nothing else. Tagged so a receipt can say so.
        return if sidecar.has_analysis_row {
            proven(Evidence::LegacyRows)
        } else {
            held("no processing record and no analysis rows")
        };
    };

    // Condition::NotFailed. Read before the conjunction, because the conjunction
    // cannot distinguish a failure from a mismatch.
    let state = record.get("state").and_then(serde_json::Value::as_str);
    if state == Some(vocab::STATE_FAILED) {
        return FileVerdict::Held(Blocker::Failed {
            name: name.as_str().to_owned(),
            state: vocab::STATE_FAILED.to_owned(),
        });
    }

    // Condition::TerminalProof, borrowed unchanged. ⛔ The size is the size ON
    // DISK: this caller is about to delete these bytes.
    match evaluate_terminal_proof(Some(record), expected_handler, size_on_disk) {
        TerminalProofOutcome::Held => {}
        TerminalProofOutcome::RecordAbsent => return held("the record is not an object"),
        TerminalProofOutcome::SchemaUnrecognized => {
            return held("the record's schema is absent or unrecognized");
        }
        TerminalProofOutcome::Refused => {
            return held("the record does not prove these bytes were consumed");
        }
    }

    // Condition::AnalyzedRowsPresent.
    if state == Some(vocab::STATE_ANALYZED) && !sidecar.has_analysis_row {
        return held(
            "the record claims analyzed and the analysis rows are gone, \
             so the raw is the only surviving copy",
        );
    }

    proven(Evidence::Record)
}

/// A segment's verdict: every content file must be releasable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RawRelease {
    Releasable(Vec<ProvenRaw>),
    Held(Vec<Blocker>),
}

/// One content file as the caller found it on disk.
pub struct FoundContent {
    pub name: ContentName,
    pub size: u64,
    pub sidecar: SidecarFacts,
}

/// Decide a whole segment.
///
/// ⛔ One blocker holds everything. The unit of the decision is the segment even
/// though the unit of the removal is the file: screen-diff frames carry no
/// extraction record of their own and depend on riding the segment's verdict, and
/// a partially-released segment is a shape no reader expects.
pub fn resolve(
    registry: &dyn HandlerRegistry,
    classifier: &dyn MediaClassifier,
    day: &str,
    stream: &str,
    dir: &str,
    found: &[FoundContent],
) -> RawRelease {
    let mut proven = Vec::new();
    let mut blockers = Vec::new();
    for item in found {
        if !classifier.is_owner_media(&item.name) {
            continue;
        }
        match resolve_file(
            registry,
            day,
            stream,
            dir,
            &item.name,
            item.size,
            &item.sidecar,
        ) {
            FileVerdict::Releasable(value) => proven.push(value),
            FileVerdict::Held(blocker) => blockers.push(blocker),
        }
    }
    if blockers.is_empty() {
        RawRelease::Releasable(proven)
    } else {
        RawRelease::Held(blockers)
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

    fn audio_only(name: &ContentName) -> Option<&'static str> {
        matches!(name.extension().as_deref(), Some("flac")).then_some("transcribe")
    }

    fn media_only(name: &ContentName) -> bool {
        matches!(
            name.extension().as_deref(),
            Some("flac" | "mp4" | "wav" | "png")
        )
    }

    fn terminal_record(size: u64) -> serde_json::Value {
        serde_json::json!({
            "schema": "solstone.processing.v1",
            "state": "empty",
            "reason_code": "no_decodable_audio",
            "handler": "transcribe",
            "attempted_at": "2026-08-05T00:00:00Z",
            "input_size": size,
        })
    }

    fn releasable(name: &str, size: u64) -> FoundContent {
        FoundContent {
            name: ContentName::new(name).unwrap(),
            size,
            sidecar: SidecarFacts {
                record: Some(terminal_record(size)),
                has_analysis_row: false,
            },
        }
    }

    /// A proof cannot be built for a name the classifier does not call media.
    ///
    /// ⛔ This is the guarantee that keeps the release verb from being asked to
    /// remove a sidecar or a derived output — not by a check that refuses, but
    /// because the value cannot be constructed. ⚠ Asserting it over reserved names
    /// or paths would measure `ContentName` instead: a **valid, non-reserved,
    /// non-media** name is the only case the classifier decides.
    #[test]
    fn a_proof_cannot_be_built_for_a_non_media_name() {
        let classifier = media_only;
        assert!(
            ProvenRaw::for_test(&classifier, "20260805", "s", "d", "audio.flac", 104).is_some()
        );
        for name in ["audio.jsonl", "notes.txt", "stream.json", "timeline.json"] {
            assert!(
                ProvenRaw::for_test(&classifier, "20260805", "s", "d", name, 104).is_none(),
                "{name} is not owner media and must not be provable"
            );
        }
    }

    #[test]
    fn a_proof_carries_the_segment_it_was_proven_in() {
        let classifier = media_only;
        let proven = ProvenRaw::for_test(
            &classifier,
            "20260805",
            "field.audio",
            "070000_17",
            "a.flac",
            9,
        )
        .unwrap();
        assert_eq!(proven.day(), "20260805");
        assert_eq!(proven.dir(), "070000_17");
        assert_eq!(
            proven.rel(),
            "chronicle/20260805/field.audio/070000_17/a.flac"
        );
    }

    /// One blocked file holds the whole segment.
    ///
    /// The removal unit is the file; the decision unit is the segment. Screen-diff
    /// frames carry no extraction record of their own and depend on riding the
    /// segment's verdict, so a partially released segment is a shape no reader
    /// expects.
    #[test]
    fn one_blocked_file_holds_the_whole_segment() {
        let registry = audio_only;
        let classifier = media_only;
        let found = vec![
            releasable("a.flac", 104),
            // No handler in the closed set claims a still image, so nothing can prove it consumed.
            FoundContent {
                name: ContentName::new("photo.png").unwrap(),
                size: 12,
                sidecar: SidecarFacts::default(),
            },
        ];
        match resolve(
            &registry,
            &classifier,
            "20260805",
            "field.audio",
            "070000_17",
            &found,
        ) {
            RawRelease::Held(blockers) => {
                assert_eq!(blockers.len(), 1);
                assert!(matches!(blockers[0], Blocker::Unprovable { .. }));
            }
            other => panic!("expected the segment held, got {other:?}"),
        }
    }

    #[test]
    fn a_segment_whose_every_file_is_proven_is_releasable() {
        let registry = audio_only;
        let classifier = media_only;
        let found = vec![releasable("a.flac", 104), releasable("b.flac", 51)];
        match resolve(
            &registry,
            &classifier,
            "20260805",
            "field.audio",
            "070000_17",
            &found,
        ) {
            RawRelease::Releasable(proven) => {
                assert_eq!(proven.len(), 2);
                assert!(proven.iter().all(|p| p.evidence() == Evidence::Record));
            }
            other => panic!("expected releasable, got {other:?}"),
        }
    }

    #[test]
    fn a_depict_record_and_text_row_still_leave_an_image_unprovable() {
        let record = serde_json::json!({
            "schema": vocab::SCHEMA,
            "state": vocab::STATE_ANALYZED,
            "reason_code": vocab::REASON_OK,
            "handler": vocab::HANDLER_DEPICT,
            "attempted_at": "2026-08-05T00:00:00Z",
            "input_size": 12,
        });
        let verdict = resolve_file(
            &crate::content::ClosedHandlerSet,
            "20260805",
            "field.audio",
            "070000_17",
            &ContentName::new("photo.png").unwrap(),
            12,
            &SidecarFacts {
                record: Some(record),
                has_analysis_row: true,
            },
        );
        assert!(
            matches!(
                verdict,
                FileVerdict::Held(Blocker::Unprovable { ref name }) if name == "photo.png"
            ),
            "image raw must stay unprovable even with a depict record and row evidence: {verdict:?}"
        );
    }

    /// A non-media file in the segment is skipped, not blocked.
    ///
    /// Sidecars and derived outputs live beside the raw and are not candidates,
    /// so they must not hold a segment that is otherwise complete.
    #[test]
    fn a_non_media_file_neither_blocks_nor_is_released() {
        let registry = audio_only;
        let classifier = media_only;
        let found = vec![
            releasable("a.flac", 104),
            FoundContent {
                name: ContentName::new("a.jsonl").unwrap(),
                size: 3,
                sidecar: SidecarFacts::default(),
            },
        ];
        match resolve(
            &registry,
            &classifier,
            "20260805",
            "field.audio",
            "070000_17",
            &found,
        ) {
            RawRelease::Releasable(proven) => {
                assert_eq!(proven.len(), 1);
                assert_eq!(proven[0].name(), "a.flac");
            }
            other => panic!("expected releasable, got {other:?}"),
        }
    }
}
