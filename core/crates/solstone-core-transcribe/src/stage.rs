// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal stage outcomes and owner-media deletion ordering.

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_speaker_id::writer::WriteResponse;

use crate::TranscribeError;
use crate::processing::{corrupt_input_record, empty_record};
use crate::terminal::{TerminalWrite, TerminalWriteFailure, write_terminal, write_terminal_with};
use crate::transcript::remove_orphan_npz;

/// Immutable facts captured from the owner-media file before processing starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputFacts {
    pub(crate) path: PathBuf,
    pub(crate) input_size: u64,
}

/// Capture the input's byte count before any terminal branch can remove it.
pub(crate) fn capture_input_facts(path: &Path) -> Result<InputFacts, TranscribeError> {
    let input_size = fs::metadata(path)
        .map_err(|error| TranscribeError::InputMetadata {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?
        .len();
    Ok(InputFacts {
        path: path.to_path_buf(),
        input_size,
    })
}

/// Terminal result that controls the transcribed event's outcome field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcome {
    /// Terminal proof was written and the raw input was deleted.
    Filtered,
    /// Terminal proof was written but preserve-all retained the raw input.
    Preserved,
    /// Decode failed, terminal failure proof was written, and input was retained.
    Failed,
}

/// Handle VAD's no-speech terminal branch.
pub(crate) fn vad_no_speech(
    facts: &InputFacts,
    preserve_all: bool,
    redo: bool,
) -> Result<TerminalOutcome, TranscribeError> {
    vad_no_speech_with(
        facts,
        preserve_all,
        redo,
        |bytes| {
            solstone_core_speaker_id::writer::write_request(bytes)
                .map_err(TerminalWriteFailure::Typed)
        },
        remove_orphan_npz,
    )
}

/// Handle STT's zero-statement terminal branch.
pub(crate) fn stt_zero_statements(
    facts: &InputFacts,
    preserve_all: bool,
    redo: bool,
) -> Result<TerminalOutcome, TranscribeError> {
    stt_zero_statements_with(
        facts,
        preserve_all,
        redo,
        |bytes| {
            solstone_core_speaker_id::writer::write_request(bytes)
                .map_err(TerminalWriteFailure::Typed)
        },
        remove_orphan_npz,
    )
}

/// Write decode failure proof without removing the original input.
pub(crate) fn decode_failure(
    facts: &InputFacts,
    redo: bool,
) -> Result<TerminalOutcome, TranscribeError> {
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    write_terminal(TerminalWrite {
        raw_path: &facts.path,
        jsonl_path: &jsonl_path,
        npz_path: &npz_path,
        processing: &corrupt_input_record(facts.input_size),
        redo,
    })?;
    Ok(TerminalOutcome::Failed)
}

pub(crate) fn transcript_paths(audio_path: &Path) -> (PathBuf, PathBuf) {
    (
        audio_path.with_extension("jsonl"),
        audio_path.with_extension("npz"),
    )
}

fn vad_no_speech_with<W, O>(
    facts: &InputFacts,
    preserve_all: bool,
    redo: bool,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    terminal_empty_then_maybe_remove(facts, preserve_all, redo, writer, orphan_remover)
}

fn stt_zero_statements_with<W, O>(
    facts: &InputFacts,
    preserve_all: bool,
    redo: bool,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    terminal_empty_then_maybe_remove(facts, preserve_all, redo, writer, orphan_remover)
}

fn terminal_empty_then_maybe_remove<W, O>(
    facts: &InputFacts,
    preserve_all: bool,
    redo: bool,
    writer: W,
    orphan_remover: O,
) -> Result<TerminalOutcome, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    let (jsonl_path, npz_path) = transcript_paths(&facts.path);
    let processing = empty_record(facts.input_size);
    write_terminal_with(
        TerminalWrite {
            raw_path: &facts.path,
            jsonl_path: &jsonl_path,
            npz_path: &npz_path,
            processing: &processing,
            redo,
        },
        writer,
        orphan_remover,
    )?;

    if preserve_all {
        return Ok(TerminalOutcome::Preserved);
    }

    fs::remove_file(&facts.path).map_err(|error| TranscribeError::RawInputRemove {
        path: facts.path.clone(),
        detail: error.to_string(),
    })?;
    Ok(TerminalOutcome::Filtered)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::DateTime;
    use serde_json::Value;
    use solstone_core_processing_record::vocab;
    use solstone_core_processing_record::{
        TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted,
    };
    use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

    use super::{
        InputFacts, TerminalOutcome, capture_input_facts, decode_failure, remove_orphan_npz,
        stt_zero_statements, stt_zero_statements_with, transcript_paths, vad_no_speech,
        vad_no_speech_with,
    };
    use crate::TranscribeError;
    use crate::terminal::TerminalWriteFailure;

    fn input(directory: &Path) -> InputFacts {
        let path = directory.join("clip.wav");
        fs::write(&path, b"owner-media").expect("input must be writable");
        capture_input_facts(&path).expect("input facts must be captured")
    }

    fn typed_payload_failure(
        _: &[u8],
    ) -> Result<solstone_core_speaker_id::writer::WriteResponse, TerminalWriteFailure> {
        Err(TerminalWriteFailure::Typed(
            SpeakerTranscriptWriteError::PayloadInvalid {
                path: "injected".to_owned(),
                detail: "injected failure".to_owned(),
            },
        ))
    }

    fn untyped_failure(
        _: &[u8],
    ) -> Result<solstone_core_speaker_id::writer::WriteResponse, TerminalWriteFailure> {
        Err(TerminalWriteFailure::Untyped {
            detail: "injected generic failure".to_owned(),
        })
    }

    #[test]
    fn vad_no_speech_typed_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = vad_no_speech_with(
            &facts,
            false,
            false,
            typed_payload_failure,
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn vad_no_speech_untyped_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = vad_no_speech_with(&facts, false, false, untyped_failure, remove_orphan_npz)
            .unwrap_err();

        assert_eq!(error.exit_code(), 1);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn stt_zero_statements_typed_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error = stt_zero_statements_with(
            &facts,
            false,
            false,
            typed_payload_failure,
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn stt_zero_statements_untyped_write_failure_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);

        let error =
            stt_zero_statements_with(&facts, false, false, untyped_failure, remove_orphan_npz)
                .unwrap_err();

        assert_eq!(error.exit_code(), 1);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
    }

    #[test]
    fn vad_no_speech_without_preserve_all_filters_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            vad_no_speech(&facts, false, false).unwrap(),
            TerminalOutcome::Filtered
        );
        assert!(!facts.path.exists());
    }

    #[test]
    fn stt_zero_statements_without_preserve_all_filters_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            stt_zero_statements(&facts, false, false).unwrap(),
            TerminalOutcome::Filtered
        );
        assert!(!facts.path.exists());
    }

    #[test]
    fn vad_no_speech_with_preserve_all_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            vad_no_speech(&facts, true, false).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(facts.path.exists());
    }

    #[test]
    fn stt_zero_statements_with_preserve_all_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());

        assert_eq!(
            stt_zero_statements(&facts, true, false).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(facts.path.exists());
    }

    #[test]
    fn payload_failure_leaves_no_terminal_artifacts_and_cleans_temporary_payload() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);
        let payload_path = std::cell::RefCell::new(None);

        let error = vad_no_speech_with(
            &facts,
            false,
            false,
            |bytes| {
                let request: Value = serde_json::from_slice(bytes).unwrap();
                *payload_path.borrow_mut() = Some(PathBuf::from(
                    request["embeddings"]["payload_path"]
                        .as_str()
                        .expect("terminal request must include payload path"),
                ));
                typed_payload_failure(bytes)
            },
            remove_orphan_npz,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 69);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(!npz_path.exists());
        assert!(
            !payload_path
                .into_inner()
                .expect("terminal writer must receive a payload path")
                .exists()
        );
        let entries = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![facts.path]);
    }

    #[test]
    fn terminal_record_holds_only_for_captured_input_size() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        assert_eq!(
            vad_no_speech(&facts, true, false).unwrap(),
            TerminalOutcome::Preserved
        );
        let header = read_header(&jsonl_path);
        let record = header
            .get("_solstone_processing")
            .expect("terminal header must include processing record");

        assert_eq!(
            evaluate_terminal_proof(Some(record), vocab::HANDLER_TRANSCRIBE, facts.input_size),
            TerminalProofOutcome::Held
        );
        assert_eq!(
            evaluate_terminal_proof(
                Some(record),
                vocab::HANDLER_TRANSCRIBE,
                facts.input_size + 1
            ),
            TerminalProofOutcome::Refused
        );
    }

    #[test]
    fn decode_failure_writes_terminal_failed_record_without_removing_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        assert_eq!(
            decode_failure(&facts, false).unwrap(),
            TerminalOutcome::Failed
        );
        assert!(facts.path.exists());
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_CORRUPT_INPUT);
        assert!(is_failure_exhausted(&record));
    }

    #[test]
    fn orphan_npz_is_removed_before_terminal_write() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (_, npz_path) = transcript_paths(&facts.path);
        fs::write(&npz_path, b"orphan").unwrap();

        assert_eq!(
            vad_no_speech(&facts, true, false).unwrap(),
            TerminalOutcome::Preserved
        );
        assert!(!npz_path.exists());
    }

    #[test]
    fn orphan_npz_removal_failure_stops_write_and_preserves_input() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, npz_path) = transcript_paths(&facts.path);
        fs::write(&npz_path, b"orphan").unwrap();
        let orphan_path = npz_path.clone();

        let error = vad_no_speech_with(
            &facts,
            false,
            false,
            |_| panic!("writer must not run after orphan removal failure"),
            move |_, _| {
                Err(TranscribeError::OrphanNpzRemove {
                    path: orphan_path,
                    detail: "injected removal failure".to_owned(),
                })
            },
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), 75);
        assert!(facts.path.exists());
        assert!(!jsonl_path.exists());
        assert!(npz_path.exists());
    }

    #[test]
    fn terminal_record_omits_attempts_and_has_parseable_attempted_at() {
        let temporary = tempfile::tempdir().unwrap();
        let facts = input(temporary.path());
        let (jsonl_path, _) = transcript_paths(&facts.path);

        vad_no_speech(&facts, true, false).unwrap();
        let record = read_header(&jsonl_path)["_solstone_processing"].clone();

        assert!(record.get("attempts").is_none());
        DateTime::parse_from_rfc3339(
            record["attempted_at"]
                .as_str()
                .expect("attempted_at must be a string"),
        )
        .expect("attempted_at must be RFC 3339");
    }

    fn read_header(path: &Path) -> Value {
        let contents = fs::read_to_string(path).expect("terminal JSONL must be readable");
        serde_json::from_str(contents.lines().next().expect("JSONL must contain header"))
            .expect("header must be JSON")
    }
}
