// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Content-free `observe.transcribed` durable event construction.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_callosum::{
    CallosumEnvelope, CallosumWriteError, DurableEvent, append_durable_event,
};
use solstone_core_observe_audio::VadResult;

use crate::TranscribeError;

const NOISY_RMS_THRESHOLD: f64 = 0.01;

/// The four terminal dispositions reported by one transcription attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscribedOutcome {
    Transcribed,
    Deferred,
    Failed,
    Preserved,
}

impl TranscribedOutcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Transcribed => "transcribed",
            Self::Deferred => "deferred",
            Self::Failed => "failed",
            Self::Preserved => "preserved",
        }
    }
}

/// A content-free accumulator: absent stages were never run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Timings(BTreeMap<String, u64>);

impl Timings {
    /// Record one stage duration, accumulating split stages such as `write`.
    pub(crate) fn add_ms(&mut self, stage: &str, milliseconds: u64) {
        let key = format!("{stage}_ms");
        *self.0.entry(key).or_default() += milliseconds;
    }

    pub(crate) fn get_ms(&self, stage: &str) -> Option<u64> {
        self.0.get(&format!("{stage}_ms")).copied()
    }

    fn as_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(key, value)| (key.clone(), Value::from(*value)))
                .collect(),
        )
    }
}

/// Inputs known at the event boundary. It intentionally contains no transcript text.
pub(crate) struct TranscribedEvent<'a> {
    pub(crate) outcome: TranscribedOutcome,
    pub(crate) input: &'a str,
    pub(crate) output: Option<&'a str>,
    pub(crate) reason: Option<&'a str>,
    pub(crate) error: Option<&'a TranscribeError>,
    pub(crate) backend: Option<&'a str>,
    pub(crate) device: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) audio_seconds: Option<f64>,
    pub(crate) reduced_seconds: Option<f64>,
    pub(crate) vad_result: Option<&'a VadResult>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) day: Option<&'a str>,
    pub(crate) segment: Option<&'a str>,
    pub(crate) observer: Option<&'a str>,
    pub(crate) timings: &'a Timings,
    pub(crate) peak_rss_mib: u64,
}

/// Construct the durable event envelope without exposing error detail or transcript content.
pub(crate) fn build_transcribed_event(input: TranscribedEvent<'_>) -> CallosumEnvelope {
    let mut extra = Map::from_iter([
        (
            "outcome".to_owned(),
            Value::String(input.outcome.label().to_owned()),
        ),
        ("input".to_owned(), Value::String(input.input.to_owned())),
        ("peak_rss_mib".to_owned(), Value::from(input.peak_rss_mib)),
        ("timings".to_owned(), input.timings.as_value()),
    ]);
    if matches!(input.outcome, TranscribedOutcome::Transcribed)
        && let Some(output) = input.output
    {
        extra.insert("output".to_owned(), Value::String(output.to_owned()));
    }
    if matches!(
        input.outcome,
        TranscribedOutcome::Deferred | TranscribedOutcome::Failed
    ) && let Some(reason) = input.reason
    {
        extra.insert("reason".to_owned(), Value::String(reason.to_owned()));
    }
    if input.outcome == TranscribedOutcome::Failed
        && let Some(error) = input.error
    {
        // This exhaustive mapping makes a new TranscribeError variant choose an
        // identifier here before it can reach the event bus.
        extra.insert(
            "error".to_owned(),
            Value::String(error_type_name(error).to_owned()),
        );
        if let TranscribeError::SpeakerAnalysis(error) = error {
            extra.extend(error.event_fields());
        }
    }
    insert_string(&mut extra, "backend", input.backend);
    insert_string(&mut extra, "device", input.device);
    insert_string(&mut extra, "model", input.model);
    if let Some(audio_seconds) = input.audio_seconds {
        extra.insert(
            "audio_seconds".to_owned(),
            Value::from(round(audio_seconds, 1)),
        );
    }
    if let Some(reduced_seconds) = input.reduced_seconds {
        extra.insert(
            "reduced_seconds".to_owned(),
            Value::from(round(reduced_seconds, 1)),
        );
    }
    if input.outcome == TranscribedOutcome::Transcribed
        && let (Some(audio_seconds), Some(asr_ms)) =
            (input.audio_seconds, input.timings.get_ms("asr"))
        && asr_ms >= 1
    {
        extra.insert(
            "rtfx".to_owned(),
            Value::from(round(audio_seconds / (asr_ms as f64 / 1_000.0), 2)),
        );
    }
    if let Some(vad) = input.vad_result {
        insert_vad_summary(&mut extra, vad);
    }
    if input.outcome == TranscribedOutcome::Transcribed
        && let Some(duration_ms) = input.duration_ms
    {
        extra.insert("duration_ms".to_owned(), Value::from(duration_ms));
    }
    insert_string(&mut extra, "day", input.day);
    insert_string(&mut extra, "segment", input.segment);
    insert_string(&mut extra, "observer", input.observer);

    CallosumEnvelope {
        tract: "observe".to_owned(),
        event: "transcribed".to_owned(),
        ts: None,
        extra,
    }
}

/// Append an already content-free transcription event to an existing segment log.
pub(crate) fn emit_transcribed_event(
    segment_path: &Path,
    event: CallosumEnvelope,
) -> Result<(), CallosumWriteError> {
    append_durable_event(segment_path, &DurableEvent::Callosum(event))
}

fn insert_string(extra: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        extra.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn insert_vad_summary(extra: &mut Map<String, Value>, vad: &VadResult) {
    extra.insert(
        "vad_duration".to_owned(),
        Value::from(round(vad.duration_s, 1)),
    );
    extra.insert(
        "vad_speech".to_owned(),
        Value::from(round(vad.speech_duration_s, 1)),
    );
    extra.insert(
        "noisy".to_owned(),
        Value::Bool(vad.is_noisy(NOISY_RMS_THRESHOLD)),
    );
    if let Some(noisy_rms) = vad.noisy_rms {
        extra.insert("noisy_rms".to_owned(), Value::from(round(noisy_rms, 4)));
        extra.insert("noisy_s".to_owned(), Value::from(round(vad.noisy_s, 1)));
    }
    if vad.loud_windows > 0 {
        extra.insert("loud_windows".to_owned(), Value::from(vad.loud_windows));
        extra.insert(
            "speech_loud_windows".to_owned(),
            Value::from(vad.speech_loud_windows),
        );
        if let Some(ratio) = vad.loud_speech_ratio() {
            extra.insert("loud_speech_ratio".to_owned(), Value::from(round(ratio, 2)));
        }
    }
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

// Do not derive this from Display: error messages can contain provider responses.
fn error_type_name(error: &TranscribeError) -> &'static str {
    match error {
        TranscribeError::ModelAsset(_) => "ModelAssetError",
        TranscribeError::SpeakerTranscriptWrite(_) => "SpeakerTranscriptWriteError",
        TranscribeError::OrphanNpzRemove { .. } => "OrphanNpzRemove",
        TranscribeError::TerminalPayload { .. } => "TerminalPayload",
        TranscribeError::TerminalWrite { .. } => "TerminalWrite",
        TranscribeError::ProcessingClaim { .. } => "ProcessingClaim",
        TranscribeError::InputMetadata { .. } => "InputMetadata",
        TranscribeError::TerminalRequest { .. } => "TerminalRequest",
        TranscribeError::TranscriptRequest { .. } => "TranscriptRequest",
        TranscribeError::TranscriptPayload { .. } => "TranscriptPayload",
        TranscribeError::VadBinary { .. } => "VadBinary",
        TranscribeError::VadTemporary { .. } => "VadTemporary",
        TranscribeError::VadHelper { .. } => "VadHelper",
        TranscribeError::VadResponse { .. } => "VadResponse",
        TranscribeError::SttSurface { .. } => "SttSurface",
        TranscribeError::ParakeetCppDeferred { .. } => "ParakeetCppDeferred",
        TranscribeError::ParakeetCppFailure { .. } => "ParakeetCppFailure",
        TranscribeError::ParakeetCoremlDeferred { .. } => "ParakeetCoremlDeferred",
        TranscribeError::ParakeetCoremlFailure { .. } => "ParakeetCoremlFailure",
        TranscribeError::ConfidentialDeferred { .. } => "ConfidentialDeferred",
        TranscribeError::SpeakerAnalysis(_) => "SpeakerAnalyzeError",
        TranscribeError::Decode { .. } => "Decode",
        TranscribeError::BackendNotImplemented { .. } => "BackendNotImplemented",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use solstone_core_observe_audio::VadResult;

    use super::{Timings, TranscribedEvent, TranscribedOutcome, build_transcribed_event};
    use crate::TranscribeError;
    use crate::speakers::SpeakerAnalyzeError;

    fn vad() -> VadResult {
        VadResult {
            duration_s: 4.0,
            speech_duration_s: 2.0,
            has_speech: true,
            speech_segments: vec![(0.0, 2.0)],
            noisy_rms: Some(0.02),
            noisy_s: 1.0,
            loud_windows: 1,
            speech_loud_windows: 1,
        }
    }

    fn input<'a>(
        outcome: TranscribedOutcome,
        timings: &'a Timings,
        error: Option<&'a TranscribeError>,
        vad_result: &'a VadResult,
    ) -> TranscribedEvent<'a> {
        TranscribedEvent {
            outcome,
            input: "chronicle/20260810/clip.wav",
            output: Some("chronicle/20260810/clip.jsonl"),
            reason: Some("test-reason"),
            error,
            backend: Some("parakeet-cpp"),
            device: Some("cpu"),
            model: Some("parakeet"),
            audio_seconds: Some(4.0),
            reduced_seconds: Some(3.0),
            vad_result: Some(vad_result),
            duration_ms: Some(42),
            day: Some("20260810"),
            segment: Some("120000_60"),
            observer: Some("test"),
            timings,
            peak_rss_mib: 12,
        }
    }

    #[test]
    fn event_field_sets_follow_each_outcome_contract() {
        let timings = Timings::default();
        let vad_result = vad();
        let failure = TranscribeError::TerminalWrite {
            detail: "test-only".to_owned(),
        };
        for outcome in [
            TranscribedOutcome::Transcribed,
            TranscribedOutcome::Deferred,
            TranscribedOutcome::Failed,
            TranscribedOutcome::Preserved,
        ] {
            let error = (outcome == TranscribedOutcome::Failed).then_some(&failure);
            let event = build_transcribed_event(input(outcome, &timings, error, &vad_result));
            assert_eq!(event.extra["outcome"], outcome.label());
            assert!(event.extra.contains_key("input"));
            assert!(event.extra.contains_key("peak_rss_mib"));
            assert!(event.extra.contains_key("timings"));
            assert_eq!(
                event.extra.contains_key("output"),
                outcome == TranscribedOutcome::Transcribed
            );
            assert_eq!(
                event.extra.contains_key("reason"),
                matches!(
                    outcome,
                    TranscribedOutcome::Deferred | TranscribedOutcome::Failed
                )
            );
            assert_eq!(
                event.extra.contains_key("error"),
                outcome == TranscribedOutcome::Failed
            );
            assert_eq!(
                event.extra.contains_key("duration_ms"),
                outcome == TranscribedOutcome::Transcribed
            );
        }
    }

    #[test]
    fn event_error_uses_only_a_stable_type_identifier() {
        let timings = Timings::default();
        let vad_result = vad();
        let error = TranscribeError::ParakeetCppFailure {
            reason: "invalid_json".to_owned(),
            detail: "SENSITIVE TRANSCRIPT CONTENT".to_owned(),
        };
        let event = build_transcribed_event(input(
            TranscribedOutcome::Failed,
            &timings,
            Some(&error),
            &vad_result,
        ));
        let error = event.extra["error"].as_str().unwrap();
        assert_eq!(error, "ParakeetCppFailure");
        assert!(!error.contains("SENSITIVE TRANSCRIPT CONTENT"));
    }

    #[test]
    fn timings_include_only_stages_that_ran() {
        let mut timings = Timings::default();
        let vad_result = vad();
        timings.add_ms("decode", 5);
        timings.add_ms("vad", 8);
        let event = build_transcribed_event(input(
            TranscribedOutcome::Preserved,
            &timings,
            None,
            &vad_result,
        ));
        let timings = event.extra["timings"].as_object().unwrap();
        assert_eq!(timings.len(), 2);
        assert_eq!(timings["decode_ms"], 5);
        assert_eq!(timings["vad_ms"], 8);
    }

    #[test]
    fn speaker_analysis_event_fields_are_reused_verbatim() {
        let timings = Timings::default();
        let vad_result = vad();
        let error = TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
            "clip.wav",
            "invoke",
            "timeout",
            Some(75),
        ));
        let event = build_transcribed_event(input(
            TranscribedOutcome::Failed,
            &timings,
            Some(&error),
            &vad_result,
        ));
        let expected = match &error {
            TranscribeError::SpeakerAnalysis(error) => error.event_fields(),
            _ => unreachable!(),
        };
        for (key, value) in expected {
            assert_eq!(event.extra.get(&key), Some(&value));
        }
        assert_eq!(
            event
                .extra
                .keys()
                .filter(|key| key.starts_with("speaker_analysis_failure_"))
                .count(),
            4
        );
    }

    #[test]
    fn event_vad_summary_is_content_free() {
        let timings = Timings::default();
        let vad_result = vad();
        let event = build_transcribed_event(input(
            TranscribedOutcome::Preserved,
            &timings,
            None,
            &vad_result,
        ));
        assert_eq!(event.extra["vad_duration"], Value::from(4.0));
        assert_eq!(event.extra["noisy"], Value::Bool(true));
    }
}
