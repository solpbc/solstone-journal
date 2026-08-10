// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Header-only terminal transcript publication.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_speaker_id::writer::{SpeakerTranscriptWriteError, WriteResponse, write_request};

use crate::TranscribeError;
use crate::transcript::remove_orphan_npz;

const REQUEST_SCHEMA: &str = "solstone-speaker-transcript-write-request-v1";
const ENCODER_ID: &str = "wespeaker-resnet34-256";

/// Input required to publish an empty or failed terminal transcript.
pub(crate) struct TerminalWrite<'a> {
    pub(crate) raw_path: &'a Path,
    pub(crate) jsonl_path: &'a Path,
    pub(crate) npz_path: &'a Path,
    pub(crate) processing: &'a Value,
    pub(crate) sound_tags: Option<&'a Value>,
    pub(crate) redo: bool,
}

/// A writer failure used by the terminal test seam.
#[derive(Debug)]
pub(crate) enum TerminalWriteFailure {
    /// The in-process speaker transcript writer's typed failure.
    Typed(SpeakerTranscriptWriteError),
    /// A non-writer failure from the write boundary.
    Untyped { detail: String },
}

impl From<TerminalWriteFailure> for TranscribeError {
    fn from(error: TerminalWriteFailure) -> Self {
        match error {
            TerminalWriteFailure::Typed(error) => Self::SpeakerTranscriptWrite(error),
            TerminalWriteFailure::Untyped { detail } => Self::TerminalWrite { detail },
        }
    }
}

/// Publish a terminal transcript through the real speaker transcript writer.
pub(crate) fn write_terminal(request: TerminalWrite<'_>) -> Result<WriteResponse, TranscribeError> {
    write_terminal_with(
        request,
        |bytes| write_request(bytes).map_err(TerminalWriteFailure::Typed),
        remove_orphan_npz,
    )
}

/// Publish a terminal transcript with replaceable writer and orphan-removal seams.
pub(crate) fn write_terminal_with<W, O>(
    request: TerminalWrite<'_>,
    writer: W,
    orphan_remover: O,
) -> Result<WriteResponse, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
{
    write_terminal_with_cleanup(request, writer, orphan_remover, remove_temporary_payload)
}

fn write_terminal_with_cleanup<W, O, C>(
    request: TerminalWrite<'_>,
    writer: W,
    orphan_remover: O,
    cleanup: C,
) -> Result<WriteResponse, TranscribeError>
where
    W: FnOnce(&[u8]) -> Result<WriteResponse, TerminalWriteFailure>,
    O: FnOnce(&Path, &Path) -> Result<(), TranscribeError>,
    C: FnOnce(&Path) -> Result<(), TranscribeError>,
{
    let payload_path = empty_payload_path()?;
    let result = (|| {
        let bytes = build_terminal_request(&request, &payload_path)?;
        // This must be the final operation before the writer is called so retry
        // cleanup cannot be skipped by a terminal publication attempt.
        orphan_remover(request.jsonl_path, request.npz_path)?;
        writer(&bytes).map_err(TranscribeError::from)
    })();
    let cleanup_result = cleanup(&payload_path);

    match result {
        Err(error) => Err(error),
        Ok(response) => {
            cleanup_result?;
            Ok(response)
        }
    }
}

fn remove_temporary_payload(payload_path: &Path) -> Result<(), TranscribeError> {
    fs::remove_file(payload_path).map_err(|error| TranscribeError::TerminalPayload {
        path: Some(payload_path.to_path_buf()),
        detail: error.to_string(),
    })
}

fn empty_payload_path() -> Result<PathBuf, TranscribeError> {
    let temporary = tempfile::Builder::new()
        .prefix("solstone-speaker-transcript-")
        .suffix(".f32le")
        .tempfile()
        .map_err(|error| TranscribeError::TerminalPayload {
            path: None,
            detail: error.to_string(),
        })?;
    let (file, path) = temporary
        .keep()
        .map_err(|error| TranscribeError::TerminalPayload {
            path: None,
            detail: error.error.to_string(),
        })?;
    drop(file);
    Ok(path)
}

fn build_terminal_request(
    request: &TerminalWrite<'_>,
    payload_path: &Path,
) -> Result<Vec<u8>, TranscribeError> {
    let raw = request
        .raw_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let mut header = Map::from_iter([
        ("raw".to_owned(), Value::String(raw.into_owned())),
        (
            "_solstone_processing".to_owned(),
            request.processing.clone(),
        ),
    ]);
    if let Some(sound_tags) = request.sound_tags {
        header.insert("sound_tags".to_owned(), sound_tags.clone());
    }
    serde_json::to_vec(&json!({
        "schema": REQUEST_SCHEMA,
        "output": {
            "jsonl_path": request.jsonl_path,
            "npz_path": request.npz_path,
            "redo": request.redo,
        },
        "base_time_us_of_day": 0,
        "source": null,
        "statements": [],
        "header": header,
        "embeddings": {
            "payload_path": payload_path,
            "payload_format": "raw-f32le-row-major-v1",
            "dtype": "float32-le",
            "shape": [0, 256],
            "byte_count": 0,
            "statement_ids": [],
            "durations_s": [],
            "encoder": ENCODER_ID,
        },
    }))
    .map_err(|error| TranscribeError::TerminalRequest {
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

    use super::{TerminalWrite, TerminalWriteFailure, write_terminal, write_terminal_with_cleanup};
    use crate::TranscribeError;

    #[test]
    fn writer_error_takes_priority_over_temporary_payload_cleanup_error() {
        let temporary = tempfile::tempdir().unwrap();
        let raw_path = temporary.path().join("clip.wav");
        let jsonl_path = temporary.path().join("clip.jsonl");
        let npz_path = temporary.path().join("clip.npz");
        fs::write(&raw_path, b"owner-media").unwrap();
        let processing = json!({"state": "empty"});

        let error = write_terminal_with_cleanup(
            TerminalWrite {
                raw_path: &raw_path,
                jsonl_path: &jsonl_path,
                npz_path: &npz_path,
                processing: &processing,
                sound_tags: None,
                redo: false,
            },
            |_| {
                Err(TerminalWriteFailure::Typed(
                    SpeakerTranscriptWriteError::PayloadInvalid {
                        path: "injected".to_owned(),
                        detail: "injected writer failure".to_owned(),
                    },
                ))
            },
            |_, _| Ok(()),
            |payload_path| {
                fs::remove_file(payload_path).unwrap();
                Err(TranscribeError::TerminalPayload {
                    path: Some(payload_path.to_path_buf()),
                    detail: "injected cleanup failure".to_owned(),
                })
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            TranscribeError::SpeakerTranscriptWrite(
                SpeakerTranscriptWriteError::PayloadInvalid { .. }
            )
        ));
        assert_eq!(error.exit_code(), 69);
    }

    #[test]
    fn terminal_header_omits_sound_tags_when_tagger_has_no_assets() {
        let temporary = tempfile::tempdir().unwrap();
        let raw_path = temporary.path().join("clip.wav");
        let jsonl_path = temporary.path().join("clip.jsonl");
        let npz_path = temporary.path().join("clip.npz");
        fs::write(&raw_path, b"owner-media").unwrap();
        let processing = json!({"state": "empty"});
        let sound_tags = crate::audio::tag_audio(&vec![0.0; 16_000], temporary.path());

        write_terminal(TerminalWrite {
            raw_path: &raw_path,
            jsonl_path: &jsonl_path,
            npz_path: &npz_path,
            processing: &processing,
            sound_tags: sound_tags.as_ref(),
            redo: false,
        })
        .unwrap();

        let header: serde_json::Value = serde_json::from_str(
            fs::read_to_string(jsonl_path)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert!(header.get("sound_tags").is_none());
    }
}
