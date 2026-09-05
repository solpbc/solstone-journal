// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::vocab;

/// The reason a record did or did not establish terminal processing proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalProofOutcome {
    Held,
    RecordAbsent,
    SchemaUnrecognized,
    Refused,
}

/// Evaluate a processing record without requiring a complete typed schema.
pub fn evaluate_terminal_proof(
    record: Option<&Value>,
    expected_handler: &str,
    input_size: u64,
) -> TerminalProofOutcome {
    let Some(record) = record.and_then(Value::as_object) else {
        return TerminalProofOutcome::RecordAbsent;
    };
    if record.get("schema").and_then(Value::as_str) != Some(vocab::SCHEMA) {
        return TerminalProofOutcome::SchemaUnrecognized;
    }
    if matches!(
        record.get("state").and_then(Value::as_str),
        Some(vocab::STATE_ANALYZED | vocab::STATE_EMPTY)
    ) && record.get("handler").and_then(Value::as_str) == Some(expected_handler)
        && record.get("input_size").and_then(Value::as_u64) == Some(input_size)
    {
        TerminalProofOutcome::Held
    } else {
        TerminalProofOutcome::Refused
    }
}

/// Return the failure-attempt count, preserving valid negative JSON integers.
pub fn record_attempts(record: &Value) -> i64 {
    // `as_i64` rejects bool/string/null/non-integral floats, matching Python's
    // integer guard including its explicit bool rejection.
    record.get("attempts").and_then(Value::as_i64).unwrap_or(0)
}

/// Return whether a failed processing record has reached terminal exhaustion.
pub fn is_failure_exhausted(record: &Value) -> bool {
    if record.get("state").and_then(Value::as_str) != Some(vocab::STATE_FAILED) {
        return false;
    }
    if record.get("reason_code").and_then(Value::as_str) == Some(vocab::REASON_CORRUPT_INPUT) {
        return true;
    }
    record_attempts(record) >= vocab::FAILED_ATTEMPT_BOUND
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted, record_attempts,
    };
    use crate::vocab;

    fn expected_outcome(value: &str) -> TerminalProofOutcome {
        match value {
            "held" => TerminalProofOutcome::Held,
            "record_absent" => TerminalProofOutcome::RecordAbsent,
            "schema_unrecognized" => TerminalProofOutcome::SchemaUnrecognized,
            "refused" => TerminalProofOutcome::Refused,
            other => panic!("unknown terminal-proof outcome {other}"),
        }
    }

    #[test]
    fn vectors_match_processing_record_predicates() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/vectors/processing-record-vectors.json"
        ))
        .expect("processing-record vectors must be JSON");

        for row in fixture["terminal_proof"]
            .as_array()
            .expect("terminal_proof must be an array")
        {
            let name = row["name"].as_str().expect("vector name must be a string");
            let expected_handler = row["expected_handler"]
                .as_str()
                .expect("expected_handler must be a string");
            let input_size = row["input_size"]
                .as_u64()
                .expect("input_size must be a u64");
            let expected = expected_outcome(
                row["expected_verdict"]
                    .as_str()
                    .expect("expected_verdict must be a string"),
            );
            assert_eq!(
                evaluate_terminal_proof(row.get("record"), expected_handler, input_size),
                expected,
                "terminal-proof vector {name}",
            );
        }

        for row in fixture["failure_exhaustion"]
            .as_array()
            .expect("failure_exhaustion must be an array")
        {
            let name = row["name"].as_str().expect("vector name must be a string");
            let record = row
                .get("record")
                .expect("failure vector record is required");
            let expected = row["expected_exhausted"]
                .as_bool()
                .expect("expected_exhausted must be a bool");
            assert_eq!(
                is_failure_exhausted(record),
                expected,
                "failure-exhaustion vector {name}",
            );
        }
    }

    #[test]
    fn unknown_record_fields_round_trip_without_loss() {
        let record = json!({
            "schema": vocab::SCHEMA,
            "state": vocab::STATE_ANALYZED,
            "handler": vocab::HANDLER_TRANSCRIBE,
            "input_size": 5,
            "operator_note": "x",
        });
        let serialized = serde_json::to_string(&record).unwrap();
        let reparsed: Value = serde_json::from_str(&serialized).unwrap();

        assert_eq!(reparsed["operator_note"], "x");
    }

    #[test]
    fn negative_attempt_count_is_preserved() {
        assert_eq!(record_attempts(&json!({"attempts": -1})), -1);
    }
}
