// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Full transcript publication and transcript-sidecar retry handling.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_observe_audio::{AudioReduction, VadResult};
use solstone_core_speaker_id::writer::{WriteResponse, write_request};

use crate::TranscribeError;
use crate::speakers::{SpeakerAnalyzeResult, SpeakerEmbeddingPayload};

const REQUEST_SCHEMA: &str = "solstone-speaker-transcript-write-request-v1";
const ENCODER_ID: &str = "wespeaker-resnet34-256";
const NOISY_RMS_THRESHOLD: f64 = 0.01;

/// Metadata supplied by the completed STT and speaker-analysis stages.
pub(crate) struct FullTranscriptWrite<'a> {
    pub(crate) raw_path: &'a Path,
    pub(crate) jsonl_path: &'a Path,
    pub(crate) npz_path: &'a Path,
    pub(crate) base_time_us_of_day: u64,
    pub(crate) source: Option<&'a str>,
    /// These statements have already been restored by the speaker-analysis handoff.
    pub(crate) speaker_result: &'a SpeakerAnalyzeResult,
    pub(crate) backend: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) device: Option<&'a str>,
    pub(crate) compute_type: Option<&'a str>,
    pub(crate) observer: Option<&'a str>,
    pub(crate) vad_result: Option<&'a VadResult>,
    pub(crate) segment_meta: Option<&'a Map<String, Value>>,
    pub(crate) overlap_detector: Option<&'a str>,
    pub(crate) speaker_evidence_version: &'a str,
    pub(crate) processing: &'a Value,
    pub(crate) sound_tags: Option<&'a Value>,
    pub(crate) speaker_analysis_producer: Option<&'a str>,
    pub(crate) redo: bool,
}

struct PreparedPayload {
    path: PathBuf,
    statement_ids: Vec<i64>,
    durations_s: Vec<f64>,
    encoder: String,
}

/// Restore STT statement and word timestamps before speaker analysis receives them.
///
/// The helper returns restored statements, and full publication consumes those returned
/// statements directly rather than applying the reduction mapping a second time.
pub(crate) fn restore_statement_timestamps(
    statements: &[Map<String, Value>],
    reduction: &AudioReduction,
) -> Vec<Map<String, Value>> {
    statements
        .iter()
        .cloned()
        .map(|mut statement| {
            restore_timestamp_field(&mut statement, "start", reduction);
            restore_timestamp_field(&mut statement, "end", reduction);
            if let Some(Value::Array(words)) = statement.get_mut("words") {
                for word in words {
                    if let Value::Object(word) = word {
                        restore_timestamp_field(word, "start", reduction);
                        restore_timestamp_field(word, "end", reduction);
                    }
                }
            }
            statement
        })
        .collect()
}

/// Publish a full transcript through the same native writer as terminal records.
pub(crate) fn write_full_transcript(
    request: FullTranscriptWrite<'_>,
) -> Result<WriteResponse, TranscribeError> {
    let payload = prepare_payload(request.speaker_result.embedding_payload.as_ref())?;
    let result = (|| {
        let bytes = build_full_request(&request, &payload)?;
        // Keep retry cleanup immediately adjacent to every publication attempt.
        remove_orphan_npz(request.jsonl_path, request.npz_path)?;
        write_request(&bytes).map_err(TranscribeError::from)
    })();
    let cleanup = remove_payload(&payload.path);

    match result {
        Err(error) => Err(error),
        Ok(response) => {
            cleanup?;
            Ok(response)
        }
    }
}

/// Remove a persisted embedding sidecar that has no matching transcript.
pub(crate) fn remove_orphan_npz(jsonl_path: &Path, npz_path: &Path) -> Result<(), TranscribeError> {
    if npz_path.is_file() && !jsonl_path.exists() {
        fs::remove_file(npz_path).map_err(|error| TranscribeError::OrphanNpzRemove {
            path: npz_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    }
    Ok(())
}

fn restore_timestamp_field(
    statement: &mut Map<String, Value>,
    key: &str,
    reduction: &AudioReduction,
) {
    if let Some(value) = statement.get(key).and_then(Value::as_f64) {
        statement.insert(
            key.to_owned(),
            Value::from(reduction.restore_timestamp(value)),
        );
    }
}

fn prepare_payload(
    embedding: Option<&SpeakerEmbeddingPayload>,
) -> Result<PreparedPayload, TranscribeError> {
    let (bytes, statement_ids, durations_s, encoder) = match embedding {
        Some(embedding) => (
            embedding.payload.as_slice(),
            embedding.statement_ids.clone(),
            embedding.durations_s.clone(),
            embedding.encoder.clone(),
        ),
        None => (&[][..], Vec::new(), Vec::new(), ENCODER_ID.to_owned()),
    };
    let temporary = tempfile::Builder::new()
        .prefix("solstone-speaker-transcript-")
        .suffix(".f32le")
        .tempfile()
        .map_err(|error| TranscribeError::TranscriptPayload {
            path: None,
            detail: error.to_string(),
        })?;
    let (mut file, path) =
        temporary
            .keep()
            .map_err(|error| TranscribeError::TranscriptPayload {
                path: None,
                detail: error.error.to_string(),
            })?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&path);
        return Err(TranscribeError::TranscriptPayload {
            path: Some(path),
            detail: error.to_string(),
        });
    }
    drop(file);
    Ok(PreparedPayload {
        path,
        statement_ids,
        durations_s,
        encoder,
    })
}

fn remove_payload(path: &Path) -> Result<(), TranscribeError> {
    fs::remove_file(path).map_err(|error| TranscribeError::TranscriptPayload {
        path: Some(path.to_path_buf()),
        detail: error.to_string(),
    })
}

fn build_full_request(
    request: &FullTranscriptWrite<'_>,
    payload: &PreparedPayload,
) -> Result<Vec<u8>, TranscribeError> {
    let statements = request
        .speaker_result
        .statements
        .iter()
        .map(native_statement)
        .collect::<Result<Vec<_>, _>>()?;
    let header = build_header(request);
    serde_json::to_vec(&json!({
        "schema": REQUEST_SCHEMA,
        "output": {
            "jsonl_path": request.jsonl_path,
            "npz_path": request.npz_path,
            "redo": request.redo,
        },
        "base_time_us_of_day": request.base_time_us_of_day,
        "source": request.source,
        "statements": statements,
        "header": header,
        "embeddings": {
            "payload_path": payload.path,
            "payload_format": "raw-f32le-row-major-v1",
            "dtype": "float32-le",
            "shape": [payload.statement_ids.len(), 256],
            "byte_count": fs::metadata(&payload.path).map_err(|error| TranscribeError::TranscriptPayload {
                path: Some(payload.path.clone()), detail: error.to_string(),
            })?.len(),
            "statement_ids": payload.statement_ids,
            "durations_s": payload.durations_s,
            "encoder": payload.encoder,
        },
    }))
    .map_err(|error| TranscribeError::TranscriptRequest {
        detail: error.to_string(),
    })
}

fn native_statement(statement: &Map<String, Value>) -> Result<Value, TranscribeError> {
    let id = statement
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_statement("id"))?;
    let start_s = statement
        .get("start")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let text = statement
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_statement("text"))?;
    let mut native = Map::from_iter([
        ("id".to_owned(), Value::from(id)),
        (
            "start_offset_us".to_owned(),
            Value::from((start_s * 1_000_000.0).round() as i64),
        ),
        ("text".to_owned(), Value::String(text.to_owned())),
    ]);
    if let Some(speaker) = statement.get("speaker") {
        native.insert("speaker".to_owned(), speaker.clone());
    }
    Ok(Value::Object(native))
}

fn invalid_statement(field: &str) -> TranscribeError {
    TranscribeError::TranscriptRequest {
        detail: format!("full transcript statement has no valid {field}"),
    }
}

fn build_header(request: &FullTranscriptWrite<'_>) -> Value {
    let mut header = Map::from_iter([
        (
            "raw".to_owned(),
            Value::String(
                request
                    .raw_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
        ),
        (
            "backend".to_owned(),
            Value::String(request.backend.unwrap_or("unknown").to_owned()),
        ),
        (
            "model".to_owned(),
            Value::String(request.model.unwrap_or("unknown").to_owned()),
        ),
        (
            "device".to_owned(),
            Value::String(request.device.unwrap_or("unknown").to_owned()),
        ),
        (
            "compute_type".to_owned(),
            Value::String(request.compute_type.unwrap_or("unknown").to_owned()),
        ),
        (
            "speaker_evidence".to_owned(),
            Value::String(request.speaker_result.speaker_evidence.clone()),
        ),
        (
            "speaker_evidence_multi_fraction".to_owned(),
            Value::from(request.speaker_result.speaker_evidence_multi_fraction),
        ),
        (
            "speaker_evidence_version".to_owned(),
            Value::String(request.speaker_evidence_version.to_owned()),
        ),
        (
            "_solstone_processing".to_owned(),
            request.processing.clone(),
        ),
    ]);
    if let Some(observer) = request.observer.filter(|value| !value.is_empty()) {
        header.insert("observer".to_owned(), Value::String(observer.to_owned()));
    }
    if let Some(vad) = request.vad_result {
        header.insert("duration".to_owned(), Value::from(vad.duration_s));
        header.insert(
            "noisy".to_owned(),
            Value::Bool(vad.is_noisy(NOISY_RMS_THRESHOLD)),
        );
        if let Some(noisy_rms) = vad.noisy_rms {
            header.insert("noisy_rms".to_owned(), Value::from(noisy_rms));
            header.insert("noisy_s".to_owned(), Value::from(vad.noisy_s));
        }
        if vad.loud_windows > 0 {
            header.insert("loud_windows".to_owned(), Value::from(vad.loud_windows));
            header.insert(
                "speech_loud_windows".to_owned(),
                Value::from(vad.speech_loud_windows),
            );
            if let Some(ratio) = vad.loud_speech_ratio() {
                header.insert("loud_speech_ratio".to_owned(), Value::from(ratio));
            }
        }
    }
    if let (Some(fraction), Some(detector)) = (
        Some(request.speaker_result.overlap_fraction),
        request.overlap_detector,
    ) {
        header.insert("overlap_fraction".to_owned(), Value::from(fraction));
        header.insert(
            "overlap_detector".to_owned(),
            Value::String(detector.to_owned()),
        );
    }
    if let Some(producer) = request.speaker_analysis_producer {
        header.insert(
            "speaker_analysis_producer".to_owned(),
            Value::String(producer.to_owned()),
        );
    }
    if let Some(segment_meta) = request.segment_meta.filter(|meta| !meta.is_empty()) {
        header.insert(
            "segment_meta".to_owned(),
            Value::Object(segment_meta.clone()),
        );
    }
    if let Some(sound_tags) = request.sound_tags {
        header.insert("sound_tags".to_owned(), sound_tags.clone());
    }
    Value::Object(header)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Map, Value, json};
    use solstone_core_observe_audio::{AudioReduction, SpeechSegment};

    use super::{FullTranscriptWrite, restore_statement_timestamps, write_full_transcript};
    use crate::processing::analyzed_record;
    use crate::speakers::{SpeakerAnalyzeResult, SpeakerEmbeddingPayload};

    fn result(statements: Vec<Map<String, Value>>) -> SpeakerAnalyzeResult {
        SpeakerAnalyzeResult {
            statements,
            embedding_payload: None,
            speaker_evidence: "none".to_owned(),
            speaker_evidence_multi_fraction: 0.0,
            overlap_fraction: 0.0,
            statement_labels: None,
        }
    }

    fn write_request<'a>(
        raw_path: &'a std::path::Path,
        jsonl_path: &'a std::path::Path,
        npz_path: &'a std::path::Path,
        result: &'a SpeakerAnalyzeResult,
        processing: &'a Value,
    ) -> FullTranscriptWrite<'a> {
        FullTranscriptWrite {
            raw_path,
            jsonl_path,
            npz_path,
            base_time_us_of_day: 0,
            source: None,
            speaker_result: result,
            backend: Some("parakeet-cpp"),
            model: Some("parakeet"),
            device: Some("cpu"),
            compute_type: Some("int8"),
            observer: None,
            vad_result: None,
            segment_meta: None,
            overlap_detector: Some("pyannote"),
            speaker_evidence_version: "speaker-evidence-v1",
            processing,
            sound_tags: None,
            speaker_analysis_producer: Some("speakers-analyze-v1"),
            redo: false,
        }
    }

    #[test]
    fn full_write_uses_timestamps_restored_before_speaker_analysis() {
        let reduction = AudioReduction {
            segments: vec![
                SpeechSegment {
                    original_start_s: 0.0,
                    original_end_s: 1.0,
                    reduced_start_s: 0.0,
                    reduced_end_s: 1.0,
                },
                SpeechSegment {
                    original_start_s: 8.0,
                    original_end_s: 9.0,
                    reduced_start_s: 3.0,
                    reduced_end_s: 4.0,
                },
            ],
            original_duration_s: 10.0,
            reduced_duration_s: 5.0,
        };
        let restored = restore_statement_timestamps(
            &[Map::from_iter([
                ("id".to_owned(), Value::from(1)),
                ("start".to_owned(), Value::from(3.0)),
                ("end".to_owned(), Value::from(3.5)),
                ("text".to_owned(), Value::String("hello".to_owned())),
                (
                    "words".to_owned(),
                    json!([{"start": 3.0, "end": 3.5, "word": "hello"}]),
                ),
            ])],
            &reduction,
        );
        assert_eq!(
            restored[0]["start"],
            Value::from(reduction.restore_timestamp(3.0))
        );
        assert_eq!(
            restored[0]["words"][0]["end"],
            Value::from(reduction.restore_timestamp(3.5))
        );

        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        fs::write(&raw, b"audio").unwrap();
        let speaker_result = result(restored);
        let processing = analyzed_record(5);
        write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();

        let lines = fs::read_to_string(jsonl).unwrap();
        let statement: Value = serde_json::from_str(lines.lines().nth(1).unwrap()).unwrap();
        assert_eq!(statement["start"], "00:00:08");
    }

    #[test]
    fn full_write_without_embeddings_uses_zero_row_payload() {
        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        fs::write(&raw, b"audio").unwrap();
        let speaker_result = result(vec![Map::from_iter([
            ("id".to_owned(), Value::from(1)),
            ("start".to_owned(), Value::from(0.0)),
            ("text".to_owned(), Value::String("hello".to_owned())),
        ])]);
        let processing = analyzed_record(5);

        let response = write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();
        assert_eq!(response.embedding_row_count, 0);
        assert!(jsonl.is_file());
        assert!(!npz.exists());
    }

    #[test]
    fn full_write_publishes_validated_speaker_embeddings() {
        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        fs::write(&raw, b"audio").unwrap();
        let mut speaker_result = result(vec![Map::from_iter([
            ("id".to_owned(), Value::from(1)),
            ("start".to_owned(), Value::from(0.0)),
            ("text".to_owned(), Value::String("hello".to_owned())),
        ])]);
        speaker_result.embedding_payload = Some(SpeakerEmbeddingPayload {
            payload: vec![0; 256 * std::mem::size_of::<f32>()],
            statement_ids: vec![1],
            durations_s: vec![0.5],
            encoder: "wespeaker-resnet34-256".to_owned(),
        });
        let processing = analyzed_record(5);

        let response = write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();
        assert_eq!(response.embedding_row_count, 1);
        assert!(npz.is_file());
    }
}
