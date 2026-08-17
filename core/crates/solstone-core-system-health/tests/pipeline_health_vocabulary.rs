// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::json;
use solstone_core_system_health::{
    CAP, DETERMINISTIC_FAILURE_REASON_CODES, MIN_SPAN_MS, SEGMENT_FLOOR_TALENTS,
    SEGMENT_NO_PROCESSING_MODALITIES, SEGMENT_NONGATING_TALENTS, SEGMENT_SUPERSEDED_TALENTS,
};

#[test]
fn vocabulary_constants_match_inlined_expectations() {
    assert_eq!(json!(SEGMENT_FLOOR_TALENTS), json!(["documents"]), "floor");
    assert_eq!(
        json!(SEGMENT_NONGATING_TALENTS),
        json!(["entities:detection"]),
        "nongating"
    );
    assert_eq!(
        SEGMENT_NO_PROCESSING_MODALITIES,
        &["markdown", "browser"],
        "no_processing"
    );
    assert_eq!(
        SEGMENT_SUPERSEDED_TALENTS,
        &[("entities", "entities:detection")],
        "superseded pairs"
    );
    assert_eq!(json!(CAP), json!(5), "cap");
    assert_eq!(MIN_SPAN_MS, 7_200_000, "min_span_ms");
    assert_eq!(
        DETERMINISTIC_FAILURE_REASON_CODES,
        &[
            "agent_stuck",
            "context_window_exceeded",
            "max_turns_exhausted",
            "model_not_found",
            "no_output",
            "non_responsive",
            "provider_request_rejected",
            "schema_invalid",
            "token_budget_exceeded",
            "wall_clock_exceeded"
        ],
        "deterministic"
    );
}
