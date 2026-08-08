// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const SEGMENT_FLOOR_TALENTS: &[&str] = &["documents"];
pub const SEGMENT_NONGATING_TALENTS: &[&str] = &["entities:detection"];
pub const SEGMENT_SUPERSEDED_TALENTS: &[(&str, &str)] = &[("entities", "entities:detection")];
pub const SEGMENT_NO_PROCESSING_MODALITIES: &[&str] = &["markdown", "browser"];
pub const CAP: usize = 5;
pub const MIN_SPAN_MS: i64 = 7_200_000;

/// Terminal subset (analyzed, purged, empty, failed-final) of Python's
/// eight-member `DataState` enum in `solstone/think/data_state.py`:
/// ANALYZED, EMPTY, PENDING, ANALYZING, FAILED, FAILED_FINAL, PURGED, ABSENT.
pub const SENSED_TERMINAL_STATES: &[&str] = &["analyzed", "purged", "empty", "failed_final"];

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
