// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
use solstone_core_speaker_id::writer::{SpeakerTranscriptWriteError, WriteResponse, write_request};

/// Coordinate the standalone transcript CLI's publication. Handler stages
/// already hold their attempt and commit guards and use the lower writer.
pub fn publish_transcript_request(
    bytes: &[u8],
) -> Result<WriteResponse, SpeakerTranscriptWriteError> {
    let malformed =
        |detail: crate::TranscribeDetail| SpeakerTranscriptWriteError::MalformedRequest { detail };
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| malformed(error.to_string().into()))?;
    let output = PathBuf::from(value["output"]["jsonl_path"].as_str().ok_or_else(|| {
        malformed(crate::TranscribeDetail::message(
            "journal.cli.transcribe.error.publish.output",
            [],
        ))
    })?);
    let output =
        std::path::absolute(output).map_err(|error| malformed(error.to_string().into()))?;
    let segment = output.parent().ok_or_else(|| {
        malformed(crate::TranscribeDetail::message(
            "journal.cli.transcribe.error.publish.parent",
            [],
        ))
    })?;
    let failure = |error: std::io::Error| SpeakerTranscriptWriteError::OutputUnwritable {
        path: output.display().to_string(),
        detail: error.to_string().into(),
    };
    let claims = OutputClaims::acquire([output.clone()], Default::default()).map_err(failure)?;
    // Match the lower writer's header precedence: segment metadata can
    // supply raw/proof fields, and an explicit nonnull proof wins last.
    let header = &value["header"];
    let metadata = &header["segment_meta"];
    let processing = header
        .get("_solstone_processing")
        .filter(|record| !record.is_null())
        .or_else(|| metadata.get("_solstone_processing"));
    let input = if processing.is_some_and(|record| !record.is_null()) {
        let name = Path::new(
            metadata
                .get("raw")
                .unwrap_or(&header["raw"])
                .as_str()
                .ok_or_else(|| {
                    malformed(crate::TranscribeDetail::message(
                        "journal.cli.transcribe.error.publish.raw",
                        [],
                    ))
                })?,
        );
        if name.file_name() != Some(name.as_os_str()) {
            return Err(malformed(crate::TranscribeDetail::message(
                "journal.cli.transcribe.error.publish.sibling",
                [],
            )));
        }
        Some(InputObservation::capture(&claims, &segment.join(name)).map_err(failure)?)
    } else {
        None
    };
    let _commit = match &input {
        Some(input) => input.commit(&claims),
        None => claims.commit(segment),
    }
    .map_err(failure)?;
    write_request(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn standalone_proof_publication_admits_input_and_refuses_deleted_or_missing_raw() {
        for mutation in ["unchanged", "deleted", "missing", "metadata_missing"] {
            let root = tempfile::tempdir().unwrap();
            let raw = root.path().join("audio.wav");
            fs::write(&raw, b"synthetic audio").unwrap();
            let payload = root.path().join("payload");
            fs::write(&payload, b"").unwrap();
            let output = raw.with_extension("jsonl");
            let mut request = json!({
                "schema": "solstone-speaker-transcript-write-request-v1",
                "output": { "jsonl_path": output, "npz_path": raw.with_extension("npz") },
                "base_time_us_of_day": 0, "statements": [],
                "header": { "raw": "audio.wav", "_solstone_processing": { "state": "empty", "handler": "transcribe", "input_size": 15 } },
                "embeddings": { "payload_path": payload, "payload_format": "raw-f32le-row-major-v1", "dtype": "float32-le", "shape": [0,256], "byte_count": 0, "statement_ids": [], "durations_s": [], "encoder": "none" }
            });
            match mutation {
                "deleted" => fs::write(root.path().join("tombstone.json"), b"{}").unwrap(),
                "missing" => fs::remove_file(&raw).unwrap(),
                "metadata_missing" => {
                    let proof = request["header"]
                        .as_object_mut()
                        .unwrap()
                        .remove("_solstone_processing")
                        .unwrap();
                    request["header"]["segment_meta"] =
                        json!({"raw": "missing.wav", "_solstone_processing": proof});
                }
                _ => (),
            }
            let result = publish_transcript_request(&serde_json::to_vec(&request).unwrap());
            if mutation == "unchanged" {
                result.unwrap();
                assert!(output.is_file());
            } else {
                assert!(result.is_err());
                assert!(!output.exists());
            }
        }
    }
}
