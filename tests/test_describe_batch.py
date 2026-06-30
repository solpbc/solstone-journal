# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for batch_preprocess_local and _preprocess_video_worker in describe.py."""

from pathlib import Path

import pytest

from solstone.observe.describe import (
    _preprocess_video_worker,
    batch_preprocess_local,
)

# Two fixture webm files — short synthetic segments, always present.
_FIXTURE_DIR = Path(__file__).parent / "fixtures/journal/chronicle/20260520/default"
_SEG_A = _FIXTURE_DIR / "090000_300" / "screen.webm"
_SEG_B = _FIXTURE_DIR / "091000_300" / "screen.webm"


@pytest.fixture(autouse=True)
def require_fixtures():
    if not _SEG_A.exists() or not _SEG_B.exists():
        pytest.skip("fixture webm files not found")


# ---------------------------------------------------------------------------
# _preprocess_video_worker — called directly (no subprocess overhead in tests)
# ---------------------------------------------------------------------------


class TestPreprocessVideoWorker:
    def test_returns_tuple_of_two(self):
        result = _preprocess_video_worker(str(_SEG_A))
        assert isinstance(result, tuple) and len(result) == 2

    def test_first_element_is_input_path_string(self):
        path_str, _ = _preprocess_video_worker(str(_SEG_A))
        assert path_str == str(_SEG_A)

    def test_second_element_is_list(self):
        _, frames = _preprocess_video_worker(str(_SEG_A))
        assert isinstance(frames, list)

    def test_no_frame_bytes_in_returned_frames(self):
        _, frames = _preprocess_video_worker(str(_SEG_A))
        for f in frames:
            assert "frame_bytes" not in f

    def test_frames_have_timestamp(self):
        _, frames = _preprocess_video_worker(str(_SEG_A))
        for f in frames:
            assert "timestamp" in f
            assert isinstance(f["timestamp"], float)

    def test_frames_have_frame_id(self):
        _, frames = _preprocess_video_worker(str(_SEG_A))
        for f in frames:
            assert "frame_id" in f

    def test_different_segments_return_different_path_strings(self):
        path_a, _ = _preprocess_video_worker(str(_SEG_A))
        path_b, _ = _preprocess_video_worker(str(_SEG_B))
        assert path_a != path_b

    def test_empty_or_nonempty_frame_list_is_valid(self):
        # Fixture segments may produce zero qualifying frames — both outcomes are valid.
        _, frames = _preprocess_video_worker(str(_SEG_A))
        assert isinstance(frames, list)


# ---------------------------------------------------------------------------
# batch_preprocess_local
# ---------------------------------------------------------------------------


class TestBatchPreprocessLocal:
    def test_returns_dict(self):
        result = batch_preprocess_local([_SEG_A], max_workers=1)
        assert isinstance(result, dict)

    def test_dict_keyed_by_path_string(self):
        result = batch_preprocess_local([_SEG_A], max_workers=1)
        assert str(_SEG_A) in result

    def test_all_input_paths_present_in_result(self):
        result = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=1)
        assert str(_SEG_A) in result
        assert str(_SEG_B) in result

    def test_result_count_matches_input_count(self):
        result = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=1)
        assert len(result) == 2

    def test_values_are_lists(self):
        result = batch_preprocess_local([_SEG_A], max_workers=1)
        assert isinstance(result[str(_SEG_A)], list)

    def test_no_frame_bytes_in_any_result(self):
        result = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=1)
        for frames in result.values():
            for f in frames:
                assert "frame_bytes" not in f

    def test_frames_have_timestamp_and_frame_id(self):
        result = batch_preprocess_local([_SEG_A], max_workers=1)
        for f in result[str(_SEG_A)]:
            assert "timestamp" in f
            assert "frame_id" in f

    def test_single_path_list(self):
        result = batch_preprocess_local([_SEG_A], max_workers=1)
        assert len(result) == 1

    def test_empty_path_list_returns_empty_dict(self):
        result = batch_preprocess_local([], max_workers=1)
        assert result == {}

    def test_max_workers_one_produces_same_keys_as_two(self):
        r1 = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=1)
        r2 = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=2)
        assert set(r1.keys()) == set(r2.keys())

    def test_max_workers_one_produces_same_frame_counts_as_two(self):
        r1 = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=1)
        r2 = batch_preprocess_local([_SEG_A, _SEG_B], max_workers=2)
        for path_str in r1:
            assert len(r1[path_str]) == len(r2[path_str])

    def test_default_max_workers_does_not_raise(self):
        # max_workers=None triggers the cpu_count()-based default
        result = batch_preprocess_local([_SEG_A])
        assert str(_SEG_A) in result
