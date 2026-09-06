// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `_solstone_processing` records for transcription outcomes.

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_processing_record::vocab;

/// Why successfully decoded audio produced no transcript.
#[derive(Clone, Copy)]
pub(crate) enum EmptyReason {
    NoSpeech,
    NoTranscript,
}

/// Preserve the distinction between the VAD and STT terminal outcomes.
pub(crate) fn empty_record(input_size: u64, reason: EmptyReason) -> Value {
    let reason_code = match reason {
        EmptyReason::NoSpeech => vocab::REASON_NO_SPEECH,
        EmptyReason::NoTranscript => vocab::REASON_NO_TRANSCRIPT,
    };
    record(vocab::STATE_EMPTY, reason_code, input_size)
}

/// Build the terminal record for an input that could not be decoded.
pub(crate) fn corrupt_input_record(input_size: u64) -> Value {
    record(vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT, input_size)
}

/// Build the completed record for a successfully analyzed input.
pub(crate) fn analyzed_record(input_size: u64) -> Value {
    record(vocab::STATE_ANALYZED, vocab::REASON_OK, input_size)
}

fn record(state: &str, reason_code: &str, input_size: u64) -> Value {
    json!({
        "schema": vocab::SCHEMA,
        "state": state,
        "reason_code": reason_code,
        "handler": vocab::HANDLER_TRANSCRIBE,
        "input_size": input_size,
        "attempted_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use solstone_core_processing_record::vocab;

    use super::{EmptyReason, analyzed_record, corrupt_input_record, empty_record};

    #[test]
    fn records_use_transcribe_vocabulary_without_attempts() {
        let records = [
            empty_record(4, EmptyReason::NoSpeech),
            empty_record(7, EmptyReason::NoTranscript),
            corrupt_input_record(5),
            analyzed_record(6),
        ];
        let expected = [
            (vocab::STATE_EMPTY, vocab::REASON_NO_SPEECH, 4),
            (vocab::STATE_EMPTY, vocab::REASON_NO_TRANSCRIPT, 7),
            (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT, 5),
            (vocab::STATE_ANALYZED, vocab::REASON_OK, 6),
        ];

        for (record, (state, reason_code, input_size)) in records.into_iter().zip(expected) {
            assert_eq!(record["schema"], vocab::SCHEMA);
            assert_eq!(record["state"], state);
            assert_eq!(record["reason_code"], reason_code);
            assert_eq!(record["handler"], vocab::HANDLER_TRANSCRIBE);
            assert_eq!(record["input_size"], input_size);
            assert!(record.get("attempts").is_none());
            DateTime::parse_from_rfc3339(
                record["attempted_at"]
                    .as_str()
                    .expect("attempted_at must be a string"),
            )
            .expect("attempted_at must be RFC 3339");
        }
    }
}
