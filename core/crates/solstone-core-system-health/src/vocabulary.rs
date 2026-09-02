// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataState {
    Analyzed,
    Empty,
    Pending,
    Analyzing,
    Failed,
    FailedFinal,
    Purged,
    Absent,
}

impl DataState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Analyzed => "analyzed",
            Self::Empty => "empty",
            Self::Pending => "pending",
            Self::Analyzing => "analyzing",
            Self::Failed => "failed",
            Self::FailedFinal => "failed_final",
            Self::Purged => "purged",
            Self::Absent => "absent",
        }
    }
}

pub const BODY_CARD_STREAMS: &[&str] = &["import.apple_health", "import.oura"];

pub const SEGMENT_FLOOR_TALENTS: &[&str] = &["documents"];
pub const SEGMENT_NONGATING_TALENTS: &[&str] = &["entities:detection"];
pub const SEGMENT_SUPERSEDED_TALENTS: &[(&str, &str)] = &[("entities", "entities:detection")];
pub const SEGMENT_NO_PROCESSING_MODALITIES: &[&str] = &["markdown", "browser"];
pub const CAP: usize = 5;
pub const MIN_SPAN_MS: i64 = 7_200_000;
pub const STUCK_FAIL_THRESHOLD: usize = 3;
pub const BACKLOG_DEFAULT_WINDOW: usize = 30;
pub const NO_SENSE_COMPLETE_AGED_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// Minimum time a not-yet-sensed modality's raw/jsonl input file must sit
/// unresolved before it counts toward backlog PENDING/STUCK state in
/// `read_backlog_view`. Independent of `NO_SENSE_COMPLETE_AGED_MS`, which
/// gates a different, already-sensed-but-not-thought-complete concern.
pub const MODALITY_INPUT_AGED_MS: i64 = 12 * 60 * 60 * 1000; // 12h: ~3x the ~4h median transcription lag

pub const WHY_FAILED: &str = "failed";
pub const WHY_CORRUPT_RAW: &str = "corrupt_raw";
pub const WHY_NEVER_ATTEMPTED: &str = "never_attempted";
pub const WHY_NO_SENSE_COMPLETE_AGED: &str = "no_sense_complete_aged";
pub const WHY_SENSED_NOT_THOUGHT: &str = "sensed_not_thought";

pub const REASON_CORRUPT_RAW: &str = "corrupt_raw";
pub const REASON_FAILING_STEP: &str = "failing_step";
pub const REASON_CATCHUP_BACKOFF: &str = "catchup_backoff";
pub const REASON_SEGMENT_REPAIR_DEGRADED: &str = "segment_repair_degraded";
pub const REASON_SEGMENT_REPAIR_PROGRESSING: &str = "segment_repair_progressing";
pub const REASON_SEGMENT_REPAIR_STUCK: &str = "segment_repair_stuck";
pub const REASON_SEGMENT_REPAIR_UNKNOWN: &str = "segment_repair_unknown";

pub const BACKLOG_STATE_COMPLETE: &str = "complete";
pub const BACKLOG_STATE_PENDING: &str = "pending";
pub const BACKLOG_STATE_STUCK: &str = "stuck";
pub const BACKLOG_STATE_UNKNOWN: &str = "unknown";

pub const SEGMENT_REPAIR_STATUS_DEGRADED: &str = "degraded";
pub const SEGMENT_REPAIR_STATUS_PROGRESSING: &str = "progressing";
pub const SEGMENT_REPAIR_STATUS_STUCK: &str = "stuck";
pub const SEGMENT_REPAIR_STATUS_UNKNOWN: &str = "unknown";

/// Terminal subset (analyzed, purged, empty, failed-final) of Python's
/// eight-member `DataState` enum in `solstone/think/data_state.py`:
/// ANALYZED, EMPTY, PENDING, ANALYZING, FAILED, FAILED_FINAL, PURGED, ABSENT.
pub const SENSED_TERMINAL_STATES: &[&str] = &["analyzed", "purged", "empty", "failed_final"];

/// Copy of `solstone_core_cogitate::DETERMINISTIC_FAILURE_REASON_CODES`.
/// Cogitate owns the list; the test below refuses drift.
pub const DETERMINISTIC_FAILURE_REASON_CODES: &[&str] = &[
    "agent_stuck",
    "context_window_exceeded",
    "max_turns_exhausted",
    "model_not_found",
    "no_output",
    "non_responsive",
    "provider_request_rejected",
    "schema_invalid",
    "token_budget_exceeded",
    "wall_clock_exceeded",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_failure_reason_codes_match_cogitate() {
        assert_eq!(
            solstone_core_cogitate::DETERMINISTIC_FAILURE_REASON_CODES.as_slice(),
            DETERMINISTIC_FAILURE_REASON_CODES
        );
    }
}
