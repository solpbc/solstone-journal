# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for frame-winnowing additions to solstone.observe.extract.

Covers:
  - _fallback_select_frames: greedy temporal-spread selection
  - _apply_category_caps: per-category hard caps (ignore / low)
  - select_frames_for_extraction: end-to-end integration of both
"""

from __future__ import annotations

from solstone.observe.extract import (
    _apply_category_caps,
    _fallback_select_frames,
    select_frames_for_extraction,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _frame(frame_id: int, timestamp: float, primary: str = "code") -> dict:
    return {
        "frame_id": frame_id,
        "timestamp": timestamp,
        "analysis": {
            "primary": primary,
            "secondary": "none",
            "overlap": False,
            "visual_description": f"frame {frame_id}",
        },
    }


def _frames_uniform(n: int, primary: str = "code", duration: float = 100.0) -> list:
    """n frames evenly spaced over `duration` seconds."""
    step = duration / max(n - 1, 1)
    return [_frame(i, i * step, primary) for i in range(n)]


# ---------------------------------------------------------------------------
# _fallback_select_frames
# ---------------------------------------------------------------------------


class TestFallbackSelectFrames:
    def test_empty_returns_empty(self):
        assert _fallback_select_frames([], 10) == []

    def test_fewer_than_max_returns_all(self):
        frames = _frames_uniform(5)
        result = _fallback_select_frames(frames, 10)
        assert sorted(result) == [f["frame_id"] for f in frames]

    def test_exact_max_returns_all(self):
        frames = _frames_uniform(10)
        result = _fallback_select_frames(frames, 10)
        assert len(result) == 10

    def test_respects_max_extractions(self):
        frames = _frames_uniform(50)
        result = _fallback_select_frames(frames, 10)
        assert len(result) == 10

    def test_first_frame_always_included(self):
        frames = _frames_uniform(30)
        result = _fallback_select_frames(frames, 5)
        assert frames[0]["frame_id"] in result

    def test_result_covers_full_duration(self):
        """Selected frames should span early, middle, and late timestamps."""
        frames = _frames_uniform(60, duration=120.0)
        result = _fallback_select_frames(frames, 10)
        timestamps = {f["frame_id"]: f["timestamp"] for f in frames}
        selected_ts = sorted(timestamps[fid] for fid in result)
        # Should span at least 80% of the recording
        assert selected_ts[-1] - selected_ts[0] >= 0.8 * 120.0

    def test_deterministic(self):
        """Same input always produces same output (no randomness)."""
        frames = _frames_uniform(40)
        r1 = _fallback_select_frames(frames, 8)
        r2 = _fallback_select_frames(frames, 8)
        assert sorted(r1) == sorted(r2)

    def test_greedy_spreads_better_than_front_loading(self):
        """Selected frames should not all cluster at the start."""
        frames = _frames_uniform(50, duration=100.0)
        result = _fallback_select_frames(frames, 5)
        timestamps = {f["frame_id"]: f["timestamp"] for f in frames}
        selected_ts = sorted(timestamps[fid] for fid in result)
        # At least one frame must be in the second half of the recording
        assert any(ts >= 50.0 for ts in selected_ts)

    def test_no_duplicate_ids(self):
        frames = _frames_uniform(30)
        result = _fallback_select_frames(frames, 10)
        assert len(result) == len(set(result))

    def test_single_frame(self):
        frames = [_frame(1, 0.0)]
        result = _fallback_select_frames(frames, 10)
        assert result == [1]

    def test_two_frames_both_selected_when_under_max(self):
        frames = [_frame(1, 0.0), _frame(2, 10.0)]
        result = _fallback_select_frames(frames, 5)
        assert set(result) == {1, 2}


# ---------------------------------------------------------------------------
# _apply_category_caps
# ---------------------------------------------------------------------------


class TestApplyCategoryCaps:
    def test_empty_selected_returns_empty(self):
        result = _apply_category_caps([], [], {})
        assert result == []

    def test_no_overrides_passes_all_through(self):
        frames = [_frame(i, float(i), "code") for i in range(5)]
        ids = [f["frame_id"] for f in frames]
        result = _apply_category_caps(ids, frames, {})
        assert result == ids

    def test_ignore_drops_all_from_that_category(self):
        frames = [
            _frame(0, 0.0, "code"),
            _frame(1, 1.0, "gaming"),
            _frame(2, 2.0, "gaming"),
            _frame(3, 3.0, "code"),
        ]
        ids = [0, 1, 2, 3]
        overrides = {"gaming": {"importance": "ignore"}}
        result = _apply_category_caps(ids, frames, overrides)
        assert 1 not in result
        assert 2 not in result
        assert 0 in result
        assert 3 in result

    def test_low_caps_at_two_frames(self):
        frames = [_frame(i, float(i), "browsing") for i in range(6)]
        ids = [f["frame_id"] for f in frames]
        overrides = {"browsing": {"importance": "low"}}
        result = _apply_category_caps(ids, frames, overrides)
        browsing_kept = [fid for fid in result]
        assert len(browsing_kept) == 2

    def test_high_importance_no_cap(self):
        frames = [_frame(i, float(i), "code") for i in range(15)]
        ids = [f["frame_id"] for f in frames]
        overrides = {"code": {"importance": "high"}}
        result = _apply_category_caps(ids, frames, overrides)
        assert len(result) == 15

    def test_normal_importance_no_cap(self):
        frames = [_frame(i, float(i), "terminal") for i in range(10)]
        ids = [f["frame_id"] for f in frames]
        overrides = {"terminal": {"importance": "normal"}}
        result = _apply_category_caps(ids, frames, overrides)
        assert len(result) == 10

    def test_mixed_categories_apply_correctly(self):
        frames = [
            _frame(0, 0.0, "code"),
            _frame(1, 1.0, "gaming"),
            _frame(2, 2.0, "gaming"),
            _frame(3, 3.0, "browsing"),
            _frame(4, 4.0, "browsing"),
            _frame(5, 5.0, "browsing"),
            _frame(6, 6.0, "code"),
        ]
        ids = [0, 1, 2, 3, 4, 5, 6]
        overrides = {
            "gaming": {"importance": "ignore"},
            "browsing": {"importance": "low"},
        }
        result = _apply_category_caps(ids, frames, overrides)
        assert 1 not in result  # gaming → ignore
        assert 2 not in result  # gaming → ignore
        # browsing capped at 2: frames 3 and 4 in, 5 out
        assert result.count(3) + result.count(4) + result.count(5) == 2
        assert 0 in result  # code → normal, no cap
        assert 6 in result  # code → normal, no cap

    def test_preserves_original_order(self):
        frames = [_frame(i, float(i), "code") for i in range(5)]
        ids = [4, 2, 0, 3, 1]
        result = _apply_category_caps(ids, frames, {})
        assert result == ids  # order preserved

    def test_unknown_category_treated_as_normal(self):
        frames = [_frame(i, float(i), "unknown_cat") for i in range(5)]
        ids = [f["frame_id"] for f in frames]
        result = _apply_category_caps(ids, frames, {})
        assert result == ids


# ---------------------------------------------------------------------------
# select_frames_for_extraction — end-to-end with caps
# ---------------------------------------------------------------------------


class TestSelectFramesWithCaps:
    def test_first_frame_survives_ignore_cap(self, monkeypatch):
        """First-frame guarantee re-adds the frame even if its category is ignored."""
        monkeypatch.setattr(
            "solstone.observe.extract._get_category_config",
            lambda: {"gaming": {"importance": "ignore"}},
        )
        frames = [_frame(0, 0.0, "gaming"), _frame(1, 5.0, "code")]
        result = select_frames_for_extraction(
            frames, max_extractions=10, categories=None
        )
        assert 0 in result  # always included as first frame

    def test_ignore_category_removed_from_non_first_frames(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.observe.extract._get_category_config",
            lambda: {"gaming": {"importance": "ignore"}},
        )
        frames = [
            _frame(0, 0.0, "code"),
            _frame(1, 5.0, "gaming"),
            _frame(2, 10.0, "gaming"),
            _frame(3, 15.0, "code"),
        ]
        result = select_frames_for_extraction(
            frames, max_extractions=10, categories=None
        )
        assert 1 not in result
        assert 2 not in result

    def test_low_category_capped_in_output(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.observe.extract._get_category_config",
            lambda: {"browsing": {"importance": "low"}},
        )
        frames = [_frame(i, float(i * 5), "browsing") for i in range(10)]
        result = select_frames_for_extraction(
            frames, max_extractions=10, categories=None
        )
        # First frame always included; remaining browsing capped at 2 → max 3 total
        assert len(result) <= 3

    def test_empty_frames_returns_empty(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.observe.extract._get_category_config",
            lambda: {},
        )
        result = select_frames_for_extraction([], max_extractions=10, categories=None)
        assert result == []
