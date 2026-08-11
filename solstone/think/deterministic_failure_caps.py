# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caps for deterministic talent failures that should not retry indefinitely."""

from __future__ import annotations

from solstone.think.responsiveness import NON_RESPONSIVE_REASON_CODE

# Reason codes for content-deterministic crashes and high-recurrence stochastic
# failures we decline to auto-retry past their caps.
DETERMINISTIC_FAILURE_REASON_CODES = frozenset(
    {
        "agent_stuck",
        "context_window_exceeded",
        "max_turns_exhausted",
        "model_not_found",
        "no_output",
        NON_RESPONSIVE_REASON_CODE,
        "provider_request_rejected",
        "schema_invalid",
        "token_budget_exceeded",
        "wall_clock_exceeded",
    }
)
# Single source of truth for deterministic failure caps consumed by
# thinking._check_daily_skip and thinking.evaluate_daily_completion. The
# judgement shape is the latest deterministic reason plus the total in-set
# deterministic count from pipeline_health.read_daily_deterministic_failures.
# Scope-provided calibration: schema_invalid measured 24.3% per-call failure on
# the affected talent (87 complete / 28 schema_invalid since the local cutover),
# with same-day fail-then-pass observed on 20260723 for entity_observer:vconic
# (failed 00:24, completed 00:36). The other reasons are unmeasured and kept
# deliberately tight.
DETERMINISTIC_FAILURE_CAPS: dict[str, int] = {
    "agent_stuck": 2,
    "context_window_exceeded": 2,
    "max_turns_exhausted": 2,
    "model_not_found": 1,
    "no_output": 2,
    NON_RESPONSIVE_REASON_CODE: 2,
    "provider_request_rejected": 1,
    "schema_invalid": 3,
    "token_budget_exceeded": 2,
    "wall_clock_exceeded": 2,
}


def failure_capped(reason_code: str | None, count: int) -> bool:
    """Return True when a deterministic failure count reaches its cap."""
    cap = DETERMINISTIC_FAILURE_CAPS.get(reason_code or "")
    return cap is not None and count >= cap
