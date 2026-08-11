# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from solstone.think import deterministic_failure_caps
from solstone.think.responsiveness import NON_RESPONSIVE_REASON_CODE


def test_failure_capped_schema_invalid_cap_is_three() -> None:
    assert deterministic_failure_caps.failure_capped("schema_invalid", 2) is False
    assert deterministic_failure_caps.failure_capped("schema_invalid", 3) is True


def test_failure_capped_default_deterministic_cap_is_two() -> None:
    assert (
        deterministic_failure_caps.failure_capped("context_window_exceeded", 1)
        is False
    )
    assert (
        deterministic_failure_caps.failure_capped("context_window_exceeded", 2)
        is True
    )


def test_failure_capped_provider_request_rejected_is_one() -> None:
    assert deterministic_failure_caps.failure_capped("provider_request_rejected", 1)


def test_failure_capped_non_responsive_cap_is_two() -> None:
    assert deterministic_failure_caps.failure_capped(NON_RESPONSIVE_REASON_CODE, 1) is False
    assert deterministic_failure_caps.failure_capped(NON_RESPONSIVE_REASON_CODE, 2) is True


def test_deterministic_failure_caps_cover_reason_codes_exactly() -> None:
    assert (
        set(deterministic_failure_caps.DETERMINISTIC_FAILURE_CAPS)
        == deterministic_failure_caps.DETERMINISTIC_FAILURE_REASON_CODES
    )
