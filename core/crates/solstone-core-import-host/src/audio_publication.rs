// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal attempt and publication for a generic audio import.
//!
//! Before this existed, a generic audio import wrote no attempt facts at all, so the web
//! reader fell to its legacy rule: `task_id` present and nothing else, which reads `Running`
//! for the wall-clock bound and then `Failed("Import never completed", "timeout")` over
//! content that is safely on disk. This records what actually happened instead.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_import::metadata::{
    AttemptState, IMPORT_FAILED_REASON, get_attempt_facts, hold_import_lock,
    read_provenance, record_completed_attempt_unlocked,
    record_completed_attempt_with_input_failures_unlocked, record_unconfirmed_attempt_unlocked,
};
use solstone_core_import::publish::{
    CreatedSegment, NativePublicationOperations, PublicationInput, PublicationOperations,
    RescanFileStatus, publish_with_operations,
};
use solstone_core_segment::{StreamAdvance, UnboundStreamAdvanceError};
use solstone_core_import::ImportError;

use crate::audio::AudioImportOutcome;

/// Publication side effects for audio.
///
/// Audio reaches publication having already run its own processing cycle, so two of the six
/// trait methods must not fire again and four must. Every method is dispositioned
/// deliberately; none is left to the default, because the trait has no defaults.
pub struct AudioPublicationOperations;

impl PublicationOperations for AudioPublicationOperations {
    /// Delegate. Audio has never advanced a stream record -- `advance_unbound_stream` has
    /// exactly one production caller, publication itself -- so this is the first and only
    /// chain advance these segments get.
    fn advance_stream(
        &self,
        journal: &Path,
        segment: &CreatedSegment,
    ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
        NativePublicationOperations.advance_stream(journal, segment)
    }

    fn rescan_file(&self, journal: &Path, path: &Path) -> Result<RescanFileStatus, String> {
        NativePublicationOperations.rescan_file(journal, path)
    }

    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
        NativePublicationOperations.touch_stream_health_marker(journal, day)
    }

    /// Suppressed. The observer already emitted `observed` for these segments during audio's
    /// own processing wait, which audio consumed to decide they were done. Re-emitting would
    /// enqueue a duplicate `journal think` per segment through `handle_segment_observed`.
    ///
    /// A stalled segment therefore gets no think pass -- it never received an `observed`
    /// either. That is intended: a stalled segment is not finished work.
    fn emit_observed(
        &self,
        _journal: &Path,
        _revision: Option<&str>,
        _day: &str,
        _segment: &str,
        _stream: &str,
    ) {
    }

    /// Delegate. Audio emits `observing` only and has never emitted this; image and PDF do.
    /// It has no consumer in this tree today, so delegating is about keeping one registered
    /// event's producer set honest rather than about anything reading it.
    fn emit_enrichment_ready(
        &self,
        journal: &Path,
        revision: Option<&str>,
        import_id: &str,
        importer: &str,
        days: &[String],
        entries_written: u64,
    ) {
        NativePublicationOperations.emit_enrichment_ready(
            journal,
            revision,
            import_id,
            importer,
            days,
            entries_written,
        );
    }

    fn emit_drain(&self, journal: &Path, revision: Option<&str>, day: &str) {
        NativePublicationOperations.emit_drain(journal, revision, day);
    }
}

/// Which terminal state this run earned, resolved as an ordered match.
///
/// The arms overlap in the producer's own types -- `Partial` is chosen purely on dropped
/// chunks while the processing outcome comes independently from the wait -- so severity
/// order is fixed here rather than left to whichever branch is tested first.
enum Terminal {
    /// Definitive failure: the row reads `failed`.
    Failed,
    /// Not final. Segments usually complete later, so the row reads `unconfirmed`.
    Unconfirmed,
    /// Content landed, with `n` inputs lost. The row reads `success` with gaps.
    SucceededWithGaps(u64),
    Succeeded,
}

fn classify(outcome: &Result<AudioImportOutcome, ImportError>) -> Terminal {
    let Ok(outcome) = outcome else {
        // Every abort path returns Err and can carry no created-segment list.
        return Terminal::Failed;
    };
    let processing = &outcome.created().processing;
    if !processing.failed_segments.is_empty() {
        return Terminal::Failed;
    }
    if !processing.stalled_segments.is_empty() {
        return Terminal::Unconfirmed;
    }
    let dropped = outcome.dropped_chunks().len() as u64;
    if dropped > 0 {
        return Terminal::SucceededWithGaps(dropped);
    }
    Terminal::Succeeded
}

/// Record the terminal attempt, and the publication record when there is content to publish.
///
/// Best effort by design: the import itself has already happened and its content is on disk.
/// A failure to record cannot unmake that, and must not turn a real import into a hard CLI
/// error -- the wall-clock bound still moves an unrecorded attempt off `Running`.
pub fn finish_audio_attempt(
    journal: &Path,
    import_id: &str,
    generation: u64,
    outcome: &Result<AudioImportOutcome, ImportError>,
) {
    let Ok(_lock) = hold_import_lock(journal, import_id) else {
        return;
    };

    // The real guard. `may_write_record` is consulted only after every stream advance,
    // marker write and event emission, so it cannot protect the stream record; this can.
    // A superseded generation writes nothing, advances nothing and emits nothing.
    let current = read_provenance(journal, import_id)
        .ok()
        .flatten()
        .and_then(|metadata| get_attempt_facts(&metadata));
    if current.map(|facts| (facts.generation, facts.state))
        != Some((generation, AttemptState::Running))
    {
        return;
    }

    let terminal = classify(outcome);
    let finished_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    if let Ok(imported) = outcome {
        let created = imported.created();
        if !created.segments.is_empty() {
            let import_dir = journal.join("imports").join(import_id);
            let may_write = || {
                read_provenance(journal, import_id)
                    .ok()
                    .flatten()
                    .and_then(|metadata| get_attempt_facts(&metadata))
                    .is_some_and(|facts| facts.generation == generation)
            };
            let _ = publish_with_operations(
                PublicationInput {
                    journal,
                    import_dir: Some(&import_dir),
                    import_id,
                    importer: "audio",
                    revision: None,
                    segments: &created.segments,
                    files_created: &created.files_created,
                    may_write_record: Some(&may_write),
                },
                &AudioPublicationOperations,
            );
        }
    }

    let _ = match terminal {
        Terminal::Failed => record_unconfirmed_attempt_unlocked(
            journal,
            import_id,
            generation,
            finished_at_ms,
            Some(IMPORT_FAILED_REASON.to_owned()),
        ),
        Terminal::Unconfirmed => record_unconfirmed_attempt_unlocked(
            journal,
            import_id,
            generation,
            finished_at_ms,
            None,
        ),
        Terminal::SucceededWithGaps(dropped) => {
            record_completed_attempt_with_input_failures_unlocked(
                journal,
                import_id,
                generation,
                finished_at_ms,
                None,
                dropped,
            )
        }
        Terminal::Succeeded => record_completed_attempt_unlocked(
            journal,
            import_id,
            generation,
            finished_at_ms,
            None,
            None,
        ),
    };
}
