# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

import pytest

from solstone.think import cogitate_policy


def test_resolve_read_scope_defaults_to_current_day_chronicle():
    assert cogitate_policy.resolve_read_scope({}, "20260427") == ["chronicle/20260427"]


def test_resolve_read_scope_expands_override_placeholders():
    assert cogitate_policy.resolve_read_scope(
        {"read_scope": ["chronicle/<day>", "chronicle/<day-2>", "facets"]},
        "20260427",
    ) == ["chronicle/20260427", "chronicle/20260425", "facets"]


def test_resolve_read_scope_span_is_inclusive():
    assert cogitate_policy.resolve_read_scope(
        {"read_scope_span": 2},
        "20260427",
    ) == ["chronicle/20260425", "chronicle/20260426", "chronicle/20260427"]


def test_cogitate_toml_removed_and_build_policy_import_fails():
    # AC 19: TOML policy generation is removed.
    policy_path = (
        Path(__file__).parents[1] / "solstone" / "think" / "policies" / "cogitate.toml"
    )
    assert not policy_path.exists()
    missing_symbol = "build" + "_per_task_policy"
    with pytest.raises(ImportError):
        exec(f"from solstone.think.cogitate_policy import {missing_symbol}", {})
