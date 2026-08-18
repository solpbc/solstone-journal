// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Talent failures that have reached a known terminal class.
/// Not the generate-path fixture and not the kebab process-health list.
pub const DETERMINISTIC_FAILURE_REASON_CODES: [&str; 10] = [
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

/// Single source of truth for deterministic failure caps.
///
/// Scope-provided calibration: schema_invalid measured 24.3% per-call failure on
/// the affected talent (87 complete / 28 schema_invalid since the local cutover),
/// with same-day fail-then-pass observed on 20260723 for entity_observer:vconic
/// (failed 00:24, completed 00:36). The other reasons are unmeasured and kept
/// deliberately tight.
pub const DETERMINISTIC_FAILURE_CAPS: [(&str, usize); 10] = [
    ("agent_stuck", 2),
    ("context_window_exceeded", 2),
    ("max_turns_exhausted", 2),
    ("model_not_found", 1),
    ("no_output", 2),
    ("non_responsive", 2),
    ("provider_request_rejected", 1),
    ("schema_invalid", 3),
    ("token_budget_exceeded", 2),
    ("wall_clock_exceeded", 2),
];

fn failure_cap(reason_code: &str) -> Option<usize> {
    DETERMINISTIC_FAILURE_CAPS
        .iter()
        .find_map(|(reason, cap)| (*reason == reason_code).then_some(*cap))
}

/// Return true when a deterministic failure count reaches its cap.
pub fn failure_capped(reason_code: Option<&str>, count: usize) -> bool {
    reason_code
        .and_then(failure_cap)
        .is_some_and(|cap| count >= cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle;

    #[test]
    fn failure_cap_vectors_match_the_oracle() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.failure_caps.len(), 61);
        for vector in &fixture.failure_caps {
            assert_eq!(
                failure_capped(vector.reason_code.as_deref(), vector.count),
                vector.expect,
                "{}",
                vector.id
            );
        }
    }
}
