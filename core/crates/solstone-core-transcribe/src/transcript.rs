// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Full transcript publication and transcript-sidecar retry handling.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
#[cfg(test)]
use solstone_core_observe_audio::AudioReduction;
use solstone_core_observe_audio::VadResult;
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
    pub(crate) sound_tag_status:
        Option<&'a solstone_core_local::install::capability_status::CapabilityStatus>,
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
#[cfg(test)]
pub(crate) fn restore_statement_timestamps(
    statements: &[Map<String, Value>],
    reduction: &AudioReduction,
) -> Vec<Map<String, Value>> {
    statements
        .iter()
        .cloned()
        .map(|mut statement| {
            restore_interval_timestamps(&mut statement, reduction);
            if let Some(Value::Array(words)) = statement.get_mut("words") {
                for word in words {
                    if let Value::Object(word) = word {
                        restore_interval_timestamps(word, reduction);
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
    let payload = prepare_payload(
        request.speaker_result.embedding_payload.as_ref(),
        request.jsonl_path,
    )?;
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

#[cfg(test)]
fn restore_interval_timestamps(interval: &mut Map<String, Value>, reduction: &AudioReduction) {
    let start = interval.get("start").and_then(Value::as_f64);
    let end = interval.get("end").and_then(Value::as_f64);
    if let Some(start) = start {
        interval.insert(
            "start".to_owned(),
            Value::from(reduction.restore_timestamp(start)),
        );
    }
    if let Some(end) = end {
        // A zero-duration word/statement remains a point, including at a cut boundary.
        let restored = if start == Some(end) {
            reduction.restore_timestamp(end)
        } else {
            reduction.restore_end_timestamp(end)
        };
        interval.insert("end".to_owned(), Value::from(restored));
    }
}

fn prepare_payload(
    embedding: Option<&SpeakerEmbeddingPayload>,
    destination: &Path,
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
    let temporary =
        tempfile::Builder::new()
            .prefix("solstone-speaker-transcript-")
            .suffix(".f32le")
            .tempfile_in(destination.parent().ok_or_else(|| {
                TranscribeError::TranscriptPayload {
                    path: Some(destination.to_path_buf()),
                    detail: "transcript destination has no parent".into(),
                }
            })?)
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
    let start_s = match statement
        .get("start")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
    {
        Some(value) => value,
        None if !statement.contains_key("end") => 0.0,
        None => return Err(invalid_statement("start")),
    };
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
    if let Some(end) = statement.get("end") {
        // Evidence stays in the original audio's seconds, independent of the
        // clock used for the existing displayed start.
        native.insert("timing".to_owned(), json!({"start":start_s,"end":end}));
    }
    if let Some(words) = statement.get("words") {
        native.insert("words".to_owned(), words.clone());
    }
    if let Some(speaker) = statement.get("speaker") {
        native.insert("speaker".to_owned(), speaker.clone());
    }
    if let Some(speaker_turn_evidence) = statement.get("speaker_turn_evidence") {
        native.insert(
            "speaker_turn_evidence".to_owned(),
            speaker_turn_evidence.clone(),
        );
    }
    if statement
        .get("turn_boundary_suspect")
        .and_then(Value::as_bool)
        == Some(true)
    {
        native.insert("turn_boundary_suspect".to_owned(), Value::Bool(true));
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
    } else if let Some(sound_tag_status) = request.sound_tag_status
        && let Ok(val) = serde_json::to_value(sound_tag_status)
    {
        header.insert("sound_tag_status".to_owned(), val);
    }
    Value::Object(header)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use serde_json::{Map, Value, json};
    use solstone_core_observe_audio::{AudioReduction, SpeechSegment, VadResult, reduce_audio};

    use super::{FullTranscriptWrite, restore_statement_timestamps, write_full_transcript};
    use crate::processing::analyzed_record;
    use crate::speakers::{
        SpeakerAnalyzeResult, SpeakerEmbeddingPayload, accepted_result_from_response,
    };
    use solstone_core_speakers::SPEAKER_TURN_EVIDENCE_VERSION;

    pub(crate) fn assert_word_persistence(
        response: &crate::backend::parakeet_cpp::TranscriptionResponse,
        expected_words: Value,
        expected_timing: Value,
        reduction: Option<&AudioReduction>,
        expected_clock: &str,
    ) {
        let statements = crate::stage::statements_from_words(&response.words);
        let restored = reduction.map_or_else(
            || statements.clone(),
            |mapping| restore_statement_timestamps(&statements, mapping),
        );
        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        let payload = temporary.path().join("payload.f32le");
        fs::write(&raw, b"audio").unwrap();
        fs::write(&payload, []).unwrap();
        let helper = helper_response(&payload, None);
        let speaker_result = accepted_result_from_response(
            &raw,
            &helper,
            &payload,
            &restored,
            &[],
            &[],
            16_000,
            20.0,
        )
        .unwrap();
        let processing = analyzed_record(5);
        let mut request = write_request(&raw, &jsonl, &npz, &speaker_result, &processing);
        request.base_time_us_of_day = 86_399_000_000;
        write_full_transcript(request).unwrap();
        let content = fs::read_to_string(jsonl).unwrap();
        let rows: Vec<Value> = content
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["start"], expected_clock);
        assert_eq!(rows[1]["text"], response.text);
        assert_eq!(rows[1]["timing"], expected_timing);
        assert_eq!(rows[1]["words"], expected_words);
        assert_eq!(rows[1]["sentence_id"], 1);
    }

    fn result(statements: Vec<Map<String, Value>>) -> SpeakerAnalyzeResult {
        SpeakerAnalyzeResult {
            statements,
            embedding_payload: None,
            speaker_evidence: "none".to_owned(),
            speaker_evidence_multi_fraction: 0.0,
            overlap_fraction: 0.0,
            statement_labels: None,
            turn_ranges: None,
            overlap_ranges: None,
        }
    }

    #[test]
    fn precise_statement_bounds_require_a_supplied_numeric_start() {
        for start in [None, Some(Value::Null), Some(json!("00:00:00"))] {
            let mut statement = json!({"id":1,"text":"hello","end":1.25});
            if let Some(start) = start {
                statement["start"] = start;
            }
            assert!(super::native_statement(statement.as_object().unwrap()).is_err());
        }
        let zero = json!({"id":1,"text":"hello","start":0.0,"end":1.25});
        let native = super::native_statement(zero.as_object().unwrap()).unwrap();
        assert_eq!(native["timing"], json!({"start":0.0,"end":1.25}));
        let untimed = json!({"id":1,"text":"hello"});
        let native = super::native_statement(untimed.as_object().unwrap()).unwrap();
        assert_eq!(native["start_offset_us"], 0);
        assert!(native.get("timing").is_none());
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
            speaker_evidence_version: "windowed-slots-v1",
            processing,
            sound_tags: None,
            sound_tag_status: None,
            speaker_analysis_producer: Some("speakers-analyze-v1"),
            redo: false,
        }
    }

    #[test]
    fn reduced_statement_and_word_intervals_follow_the_samples_on_both_sides_of_a_cut() {
        let audio = (0..7 * 16_000)
            .map(|sample| sample as f32)
            .collect::<Vec<_>>();
        let vad = VadResult {
            duration_s: 7.0,
            speech_duration_s: 2.0,
            has_speech: true,
            speech_segments: vec![(0.0, 1.0), (5.0, 6.0)],
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        let (reduced, mapping) = reduce_audio(&audio, &vad).unwrap();
        assert_eq!(&reduced[..32_000], &audio[..32_000]);
        assert_eq!(&reduced[32_000..], &audio[64_000..]);
        let intervals = [(1.5, 2.0), (2.0, 2.5), (1.75, 2.25), (2.0, 2.0)];
        let statements = intervals
            .iter()
            .map(|&(start, end)| {
                let Value::Object(statement) = json!({
                    "start": start,
                    "end": end,
                    "words": [{"start": start, "end": end, "word": "sample"}],
                }) else {
                    unreachable!()
                };
                statement
            })
            .collect::<Vec<_>>();
        let restored = restore_statement_timestamps(&statements, &mapping);
        for (index, (expected_start, expected_end)) in
            [(1.5, 2.0), (4.0, 4.5), (1.75, 4.25), (4.0, 4.0)]
                .into_iter()
                .enumerate()
        {
            assert_eq!(restored[index]["start"], expected_start);
            assert_eq!(restored[index]["end"], expected_end);
            assert_eq!(restored[index]["words"][0]["start"], expected_start);
            assert_eq!(restored[index]["words"][0]["end"], expected_end);
        }
        // Restoring clones leaves the ASR's reduced-coordinate response untouched.
        assert_eq!(statements[0]["end"], 2.0);
        assert_eq!(statements[1]["start"], 2.0);
    }

    #[test]
    fn full_write_uses_timestamps_restored_before_speaker_analysis() {
        let reduction = AudioReduction {
            segments: vec![
                SpeechSegment {
                    original_start_s: 0.0,
                    original_end_s: 2.0,
                    reduced_start_s: 0.0,
                    reduced_end_s: 2.0,
                },
                SpeechSegment {
                    original_start_s: 7.0,
                    original_end_s: 10.0,
                    reduced_start_s: 2.0,
                    reduced_end_s: 5.0,
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
                    json!([{"start": 3.0, "end": 3.5, "word": "hello", "probability":null}]),
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
        assert_eq!(statement["timing"], json!({"start":8.0,"end":8.5}));
        assert_eq!(
            statement["words"],
            json!([{"start":8.0,"end":8.5,"word":"hello","probability":null}])
        );
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

    fn helper_response(payload: &std::path::Path, turns: Option<Value>) -> Value {
        let mut pyannote = json!({"window_stats": []});
        if let Some(turns) = turns {
            pyannote["speaker_turns"] = turns;
        }
        json!({
            "schema": "solstone-speaker-analyze-response-v1",
            "sample_rate_hz": 16_000,
            "inputs": {
                "statement_embedding": {"statement_ids": [], "spans_s": []},
                "diarization": {"statement_ids": [], "spans_s": []},
            },
            "statement_embeddings": {
                "audio_buffer": "full",
                "encoder": "wespeaker-resnet34-256",
                "payload_format": "raw-f32le-row-major-v1",
                "payload_path": payload,
                "dtype": "float32-le",
                "statement_ids": [],
                "durations_s": [],
                "shape": [0, 256],
                "byte_count": 0,
                "admitted_count": 0,
                "skipped_count": 0,
            },
            "pyannote": pyannote,
            "evidence": {
                "speaker_evidence": "none",
                "multi_window_fraction": 0.0,
                "mean_window_overlap_share": 0.0,
                "overlap_fraction": 0.0,
            },
            "diarization": {
                "intervals": null,
                "valid_intervals": null,
                "interval_embeddings": null,
                "cluster_labels": null,
                "statement_labels": null,
                "silhouette_k": null,
                "effective_k": null,
            },
        })
    }

    #[test]
    fn connected_persistence_reduction_flags_restored_sentence_and_preserves_evidence() {
        let reduction = AudioReduction {
            segments: vec![SpeechSegment {
                original_start_s: 10.0,
                original_end_s: 14.0,
                reduced_start_s: 0.0,
                reduced_end_s: 4.0,
            }],
            original_duration_s: 20.0,
            reduced_duration_s: 4.0,
        };
        let restored = restore_statement_timestamps(
            &[Map::from_iter([
                ("id".to_owned(), Value::from(1)),
                ("start".to_owned(), Value::from(0.0)),
                ("end".to_owned(), Value::from(4.0)),
                (
                    "text".to_owned(),
                    Value::String("Is the gate open? It is open.".to_owned()),
                ),
                ("speaker".to_owned(), json!(1)),
            ])],
            &reduction,
        );

        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        let payload = temporary.path().join("payload.f32le");
        fs::write(&raw, b"audio").unwrap();
        fs::write(&payload, []).unwrap();

        let turns = json!({
            "version": SPEAKER_TURN_EVIDENCE_VERSION,
            "audio_duration_s": 20.0,
            "ranges": [{"start_s": 11.0, "end_s": 12.0}],
        });
        let response = helper_response(&payload, Some(turns));
        let speaker_result = accepted_result_from_response(
            &raw,
            &response,
            &payload,
            &restored,
            &[],
            &[],
            16_000,
            20.0,
        )
        .unwrap();

        let processing = analyzed_record(5);
        write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();

        let content = fs::read_to_string(&jsonl).unwrap();
        let sentence_line = content.lines().nth(1).unwrap();
        let parsed: Value = serde_json::from_str(sentence_line).unwrap();

        let keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec![
                "start",
                "text",
                "speaker",
                "sentence_id",
                "timing",
                "speaker_turn_evidence",
                "turn_boundary_suspect",
            ]
        );
        assert_eq!(parsed["start"], "00:00:10");
        assert_eq!(parsed["text"], "Is the gate open? It is open.");
        assert_eq!(parsed["speaker"], 1);
        assert_eq!(parsed["sentence_id"], 1);
        assert_eq!(parsed["turn_boundary_suspect"], true);
        assert_eq!(parsed["speaker_turn_evidence"]["audio_duration_s"], 20.0);
        assert_eq!(
            parsed["speaker_turn_evidence"]["ranges"],
            json!([{"start_s": 11.0, "end_s": 12.0}])
        );
    }

    #[test]
    fn connected_persistence_measured_empty_ranges_persists_evidence_without_boolean() {
        let statements = vec![Map::from_iter([
            ("id".to_owned(), Value::from(1)),
            ("start".to_owned(), Value::from(0.0)),
            ("end".to_owned(), Value::from(4.0)),
            (
                "text".to_owned(),
                Value::String("Is the gate open? It is open.".to_owned()),
            ),
            ("speaker".to_owned(), json!(1)),
        ])];

        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        let payload = temporary.path().join("payload.f32le");
        fs::write(&raw, b"audio").unwrap();
        fs::write(&payload, []).unwrap();

        let turns = json!({
            "version": SPEAKER_TURN_EVIDENCE_VERSION,
            "audio_duration_s": 20.0,
            "ranges": [],
        });
        let response = helper_response(&payload, Some(turns));
        let speaker_result = accepted_result_from_response(
            &raw,
            &response,
            &payload,
            &statements,
            &[],
            &[],
            16_000,
            20.0,
        )
        .unwrap();

        let processing = analyzed_record(5);
        write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();

        let content = fs::read_to_string(&jsonl).unwrap();
        let sentence_line = content.lines().nth(1).unwrap();
        let parsed: Value = serde_json::from_str(sentence_line).unwrap();

        assert_eq!(parsed["speaker_turn_evidence"]["ranges"], json!([]));
        assert!(parsed.get("turn_boundary_suspect").is_none());
    }

    #[test]
    fn connected_persistence_old_helper_without_speaker_turns_omits_both_keys() {
        let statements = vec![Map::from_iter([
            ("id".to_owned(), Value::from(1)),
            ("start".to_owned(), Value::from(0.0)),
            ("end".to_owned(), Value::from(4.0)),
            (
                "text".to_owned(),
                Value::String("Is the gate open? It is open.".to_owned()),
            ),
            ("speaker".to_owned(), json!(1)),
        ])];

        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        let payload = temporary.path().join("payload.f32le");
        fs::write(&raw, b"audio").unwrap();
        fs::write(&payload, []).unwrap();

        let response = helper_response(&payload, None);
        let speaker_result = accepted_result_from_response(
            &raw,
            &response,
            &payload,
            &statements,
            &[],
            &[],
            16_000,
            20.0,
        )
        .unwrap();

        let processing = analyzed_record(5);
        write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();

        let content = fs::read_to_string(&jsonl).unwrap();
        let sentence_line = content.lines().nth(1).unwrap();
        let parsed: Value = serde_json::from_str(sentence_line).unwrap();

        assert!(parsed.get("speaker_turn_evidence").is_none());
        assert!(parsed.get("turn_boundary_suspect").is_none());
    }

    #[test]
    fn connected_persistence_boundary_turn_range_at_junction_omits_boolean_for_both() {
        let statements = vec![
            Map::from_iter([
                ("id".to_owned(), Value::from(1)),
                ("start".to_owned(), Value::from(0.0)),
                ("end".to_owned(), Value::from(5.0)),
                (
                    "text".to_owned(),
                    Value::String("Is the gate open? It is open.".to_owned()),
                ),
                ("speaker".to_owned(), json!(1)),
            ]),
            Map::from_iter([
                ("id".to_owned(), Value::from(2)),
                ("start".to_owned(), Value::from(5.0)),
                ("end".to_owned(), Value::from(10.0)),
                (
                    "text".to_owned(),
                    Value::String("Is the gate open? It is open.".to_owned()),
                ),
                ("speaker".to_owned(), json!(1)),
            ]),
        ];

        let temporary = tempfile::tempdir().unwrap();
        let raw = temporary.path().join("clip.wav");
        let jsonl = temporary.path().join("clip.jsonl");
        let npz = temporary.path().join("clip.npz");
        let payload = temporary.path().join("payload.f32le");
        fs::write(&raw, b"audio").unwrap();
        fs::write(&payload, []).unwrap();

        let turns = json!({
            "version": SPEAKER_TURN_EVIDENCE_VERSION,
            "audio_duration_s": 10.0,
            "ranges": [{"start_s": 5.0, "end_s": 5.0}],
        });
        let response = helper_response(&payload, Some(turns));
        let speaker_result = accepted_result_from_response(
            &raw,
            &response,
            &payload,
            &statements,
            &[],
            &[],
            16_000,
            10.0,
        )
        .unwrap();

        let processing = analyzed_record(5);
        write_full_transcript(write_request(
            &raw,
            &jsonl,
            &npz,
            &speaker_result,
            &processing,
        ))
        .unwrap();

        let content = fs::read_to_string(&jsonl).unwrap();
        let sentence1: Value = serde_json::from_str(content.lines().nth(1).unwrap()).unwrap();
        let sentence2: Value = serde_json::from_str(content.lines().nth(2).unwrap()).unwrap();

        assert!(sentence1.get("speaker_turn_evidence").is_some());
        assert!(sentence1.get("turn_boundary_suspect").is_none());
        assert!(sentence2.get("speaker_turn_evidence").is_some());
        assert!(sentence2.get("turn_boundary_suspect").is_none());
    }
}
