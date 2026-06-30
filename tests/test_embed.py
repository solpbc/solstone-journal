# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for solstone.observe.embed — MobileViT-small frame embedding.

All tests run without the actual ONNX model file. Tests that exercise
embed_frames() mock the InferenceSession; tests for cluster_and_select()
and _preprocess() use pure numpy / PIL and need no model at all.
"""

from __future__ import annotations

import io
from unittest.mock import MagicMock, patch

import numpy as np
import pytest
from PIL import Image

from solstone.observe.embed import (
    DEFAULT_MIN_FRAMES,
    EMBEDDING_DIM,
    IMAGENET_MEAN,
    IMAGENET_STD,
    INPUT_SIZE,
    MOBILEVIT_MODEL_PATH,
    _preprocess,
    cluster_and_select,
    embed_frames,
    is_available,
    reduce_frames,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _png_bytes(
    r: int = 128, g: int = 128, b: int = 128, size: tuple = (320, 240)
) -> bytes:
    img = Image.new("RGB", size, color=(r, g, b))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def _frame(
    frame_id: int = 1,
    timestamp: float = 0.0,
    r: int = 128,
    g: int = 128,
    b: int = 128,
    scene_cut: bool = False,
) -> dict:
    f = {
        "frame_id": frame_id,
        "timestamp": timestamp,
        "frame_bytes": _png_bytes(r, g, b),
    }
    if scene_cut:
        f["scene_cut"] = True
    return f


def _mock_session(dim: int = EMBEDDING_DIM, fixed_vec: np.ndarray | None = None):
    """Return a mock InferenceSession that emits a fixed or random embedding."""
    session = MagicMock()
    session.get_inputs.return_value = [MagicMock(name="pixel_values")]
    pooler = MagicMock()
    pooler.name = "pooler_output"
    session.get_outputs.return_value = [pooler]

    def _run(output_names, input_feed):
        vec = (
            fixed_vec
            if fixed_vec is not None
            else np.random.rand(1, dim).astype(np.float32)
        )
        return [vec]

    session.run.side_effect = _run
    return session


def _identical_embeddings(n: int, dim: int = EMBEDDING_DIM) -> np.ndarray:
    base = np.ones((1, dim), dtype=np.float32)
    base /= np.linalg.norm(base)
    return np.repeat(base, n, axis=0)


def _orthogonal_embeddings(n: int, dim: int = EMBEDDING_DIM) -> np.ndarray:
    """n orthogonal unit vectors — each maximally far from the others."""
    e = np.zeros((n, dim), dtype=np.float32)
    for i in range(n):
        e[i, i % dim] = 1.0
    return e


# ---------------------------------------------------------------------------
# _preprocess
# ---------------------------------------------------------------------------


class TestPreprocess:
    def test_output_shape(self):
        tensor = _preprocess(_png_bytes())
        assert tensor.shape == (1, 3, INPUT_SIZE[1], INPUT_SIZE[0])

    def test_dtype_float32(self):
        tensor = _preprocess(_png_bytes())
        assert tensor.dtype == np.float32

    def test_resizes_non_square_input(self):
        # Non-square source PNG
        tensor = _preprocess(_png_bytes(size=(1920, 1080)))
        assert tensor.shape == (1, 3, INPUT_SIZE[1], INPUT_SIZE[0])

    def test_red_channel_normalised_correctly(self):
        # Pure red frame: pixel R=255 → scaled to 1.0 → normalised by ImageNet stats
        tensor = _preprocess(_png_bytes(r=255, g=0, b=0))
        expected_r = (1.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0]
        assert abs(float(tensor[0, 0, 0, 0]) - expected_r) < 0.02

    def test_zero_pixel_channel_is_negative(self):
        # G=0 channel should normalise to a negative value (0 < mean)
        tensor = _preprocess(_png_bytes(r=255, g=0, b=0))
        assert tensor[0, 1, 0, 0] < 0  # green channel

    def test_grey_frame_channels_match_expected_normalisation(self):
        tensor = _preprocess(_png_bytes(r=128, g=128, b=128))
        # Per-channel ImageNet normalisation produces distinct values (different mean/std)
        pixel = 128 / 255.0
        for ch in range(3):
            expected = (pixel - float(IMAGENET_MEAN[ch])) / float(IMAGENET_STD[ch])
            assert abs(float(tensor[0, ch, 0, 0]) - expected) < 0.02


# ---------------------------------------------------------------------------
# is_available
# ---------------------------------------------------------------------------


class TestIsAvailable:
    def test_false_when_model_missing(self):
        # In CI the model file is absent
        assert is_available() == MOBILEVIT_MODEL_PATH.is_file()

    def test_true_when_model_present(self, tmp_path, monkeypatch):
        fake = tmp_path / "mobilevit-small.onnx"
        fake.touch()
        monkeypatch.setattr("solstone.observe.embed.MOBILEVIT_MODEL_PATH", fake)
        import solstone.observe.embed as embed_mod

        assert embed_mod.is_available() is True

    def test_false_when_model_absent(self, tmp_path, monkeypatch):
        missing = tmp_path / "mobilevit-small.onnx"
        monkeypatch.setattr("solstone.observe.embed.MOBILEVIT_MODEL_PATH", missing)
        import solstone.observe.embed as embed_mod

        assert embed_mod.is_available() is False


# ---------------------------------------------------------------------------
# embed_frames
# ---------------------------------------------------------------------------


class TestEmbedFrames:
    @patch("solstone.observe.embed._get_session")
    def test_returns_correct_shape(self, mock_get):
        mock_get.return_value = _mock_session()
        frames = [_frame(i) for i in range(5)]
        embs = embed_frames(frames)
        assert embs.shape == (5, EMBEDDING_DIM)
        assert embs.dtype == np.float32

    @patch("solstone.observe.embed._get_session")
    def test_empty_input_returns_empty(self, mock_get):
        mock_get.return_value = _mock_session()
        embs = embed_frames([])
        assert embs.shape == (0, EMBEDDING_DIM)

    @patch("solstone.observe.embed._get_session")
    def test_session_called_once_per_frame(self, mock_get):
        session = _mock_session()
        mock_get.return_value = session
        frames = [_frame(i) for i in range(4)]
        embed_frames(frames)
        assert session.run.call_count == 4

    @patch("solstone.observe.embed._get_session")
    def test_single_frame(self, mock_get):
        mock_get.return_value = _mock_session()
        embs = embed_frames([_frame(1)])
        assert embs.shape == (1, EMBEDDING_DIM)

    @patch("solstone.observe.embed._get_session")
    def test_oversized_model_output_truncated(self, mock_get):
        # Model returns a vector larger than EMBEDDING_DIM — should truncate
        session = _mock_session(dim=512)
        mock_get.return_value = session
        embs = embed_frames([_frame(1)])
        assert embs.shape == (1, EMBEDDING_DIM)

    def test_raises_when_model_missing(self, monkeypatch, tmp_path):
        missing = tmp_path / "mobilevit-small.onnx"
        monkeypatch.setattr("solstone.observe.embed.MOBILEVIT_MODEL_PATH", missing)
        import solstone.observe.embed as embed_mod

        embed_mod._session = None  # reset cache
        with pytest.raises(FileNotFoundError, match="mobilevit"):
            embed_frames([_frame(1)])
        embed_mod._session = None  # clean up


# ---------------------------------------------------------------------------
# cluster_and_select
# ---------------------------------------------------------------------------


class TestClusterAndSelect:
    def test_empty_input(self):
        result = cluster_and_select([], np.empty((0, EMBEDDING_DIM), dtype=np.float32))
        assert result == []

    def test_single_frame_always_kept(self):
        frames = [_frame(1)]
        embs = _identical_embeddings(1)
        result = cluster_and_select(frames, embs)
        assert len(result) == 1
        assert result[0]["frame_id"] == 1

    def test_first_frame_always_kept(self):
        frames = [_frame(i) for i in range(10)]
        embs = _identical_embeddings(10)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        assert result[0]["frame_id"] == 0

    def test_identical_embeddings_collapse_to_one(self):
        frames = [_frame(i) for i in range(8)]
        embs = _identical_embeddings(8)
        # min_frames=1 isolates clustering logic from the floor
        result = cluster_and_select(frames, embs, distance_threshold=0.10, min_frames=1)
        assert len(result) == 1

    def test_orthogonal_embeddings_all_kept(self):
        n = 6
        frames = [_frame(i) for i in range(n)]
        embs = _orthogonal_embeddings(n)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        assert len(result) == n

    def test_scene_cut_always_kept_regardless_of_similarity(self):
        frames = [
            _frame(0, scene_cut=False),
            _frame(1, scene_cut=True),  # identical embedding but scene_cut
            _frame(2, scene_cut=False),
        ]
        embs = _identical_embeddings(3)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        ids = [f["frame_id"] for f in result]
        assert 0 in ids
        assert 1 in ids

    def test_multiple_scene_cuts_all_kept(self):
        frames = [_frame(i, scene_cut=(i % 3 == 0 and i > 0)) for i in range(9)]
        embs = _identical_embeddings(9)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        # frame 0 (first), frames 3 and 6 (scene cuts)
        ids = [f["frame_id"] for f in result]
        assert 0 in ids
        assert 3 in ids
        assert 6 in ids

    def test_result_preserves_original_order(self):
        n = 8
        frames = [_frame(i) for i in range(n)]
        embs = _orthogonal_embeddings(n)
        result = cluster_and_select(frames, embs)
        ids = [f["frame_id"] for f in result]
        assert ids == sorted(ids)

    def test_higher_threshold_gives_fewer_frames(self):
        np.random.seed(0)
        n = 30
        frames = [_frame(i) for i in range(n)]
        embs = np.random.randn(n, EMBEDDING_DIM).astype(np.float32)
        tight = cluster_and_select(frames, embs, distance_threshold=0.05)
        loose = cluster_and_select(frames, embs, distance_threshold=0.60)
        assert len(tight) >= len(loose)

    def test_lower_threshold_gives_more_frames(self):
        np.random.seed(1)
        n = 20
        frames = [_frame(i) for i in range(n)]
        embs = np.random.randn(n, EMBEDDING_DIM).astype(np.float32)
        more = cluster_and_select(frames, embs, distance_threshold=0.05)
        fewer = cluster_and_select(frames, embs, distance_threshold=0.80)
        assert len(more) >= len(fewer)

    def test_mismatched_lengths_raises(self):
        frames = [_frame(i) for i in range(3)]
        embs = np.ones((5, EMBEDDING_DIM), dtype=np.float32)
        with pytest.raises(ValueError, match="same length"):
            cluster_and_select(frames, embs)

    def test_zero_norm_embeddings_handled(self):
        # All-zero embeddings should not crash (div-by-zero guard)
        frames = [_frame(i) for i in range(3)]
        embs = np.zeros((3, EMBEDDING_DIM), dtype=np.float32)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        assert len(result) >= 1

    def test_returned_frames_are_original_dicts(self):
        frames = [_frame(i) for i in range(3)]
        embs = _orthogonal_embeddings(3)
        result = cluster_and_select(frames, embs)
        for r in result:
            assert r in frames

    def test_min_frames_floor_applied_when_clustering_too_aggressive(self):
        # Identical embeddings → clustering gives 1; floor should lift to min_frames.
        frames = [_frame(i, timestamp=float(i * 10)) for i in range(10)]
        embs = _identical_embeddings(10)
        result = cluster_and_select(frames, embs, distance_threshold=0.10, min_frames=5)
        assert len(result) == 5

    def test_min_frames_floor_clips_to_available_frames(self):
        # min_frames > len(frames) → return all frames, not more.
        frames = [_frame(i, timestamp=float(i)) for i in range(3)]
        embs = _identical_embeddings(3)
        result = cluster_and_select(
            frames, embs, distance_threshold=0.10, min_frames=10
        )
        assert len(result) == 3

    def test_min_frames_floor_fills_by_temporal_spread(self):
        # 10 identical frames at evenly-spaced timestamps; clustering → 1 frame.
        # Floor at 5 should pick frames spread across the full duration.
        frames = [_frame(i, timestamp=float(i * 10)) for i in range(10)]
        embs = _identical_embeddings(10)
        result = cluster_and_select(frames, embs, distance_threshold=0.10, min_frames=5)
        assert len(result) == 5
        timestamps = sorted(f["timestamp"] for f in result)
        # Should span at least 60% of the 90-second range (0..90)
        assert timestamps[-1] - timestamps[0] >= 0.6 * 90.0

    def test_min_frames_not_applied_when_clustering_gives_enough(self):
        # Orthogonal embeddings → all frames kept; floor has no effect.
        n = 8
        frames = [_frame(i, timestamp=float(i)) for i in range(n)]
        embs = _orthogonal_embeddings(n)
        result = cluster_and_select(frames, embs, distance_threshold=0.10, min_frames=3)
        assert len(result) == n

    def test_min_frames_result_in_temporal_order(self):
        frames = [_frame(i, timestamp=float(i * 5)) for i in range(10)]
        embs = _identical_embeddings(10)
        result = cluster_and_select(frames, embs, distance_threshold=0.10, min_frames=5)
        ts = [f["timestamp"] for f in result]
        assert ts == sorted(ts)

    def test_default_min_frames_constant_applied(self):
        # Without explicit min_frames, the default should be DEFAULT_MIN_FRAMES.
        frames = [_frame(i, timestamp=float(i * 10)) for i in range(20)]
        embs = _identical_embeddings(20)
        result = cluster_and_select(frames, embs, distance_threshold=0.10)
        assert len(result) == DEFAULT_MIN_FRAMES


# ---------------------------------------------------------------------------
# reduce_frames
# ---------------------------------------------------------------------------


class TestReduceFrames:
    def test_empty_returns_empty(self):
        assert reduce_frames([]) == []

    def test_fallback_when_model_absent(self, monkeypatch):
        monkeypatch.setattr("solstone.observe.embed.is_available", lambda: False)
        frames = [_frame(i) for i in range(5)]
        result = reduce_frames(frames)
        assert result is frames  # identical object — no copy, no mutation

    @patch("solstone.observe.embed.is_available", return_value=True)
    @patch("solstone.observe.embed._get_session")
    def test_reduces_identical_frames(self, mock_get, _mock_avail):
        vec = np.ones((1, EMBEDDING_DIM), dtype=np.float32)
        mock_get.return_value = _mock_session(fixed_vec=vec)
        frames = [_frame(i) for i in range(6)]
        # min_frames=1 isolates reduction logic from the floor
        result = reduce_frames(frames, distance_threshold=0.10, min_frames=1)
        assert len(result) == 1

    @patch("solstone.observe.embed.is_available", return_value=True)
    @patch("solstone.observe.embed.embed_frames", side_effect=RuntimeError("boom"))
    def test_fallback_on_embed_failure(self, _mock_embed, _mock_avail):
        frames = [_frame(i) for i in range(4)]
        result = reduce_frames(frames)
        assert result is frames

    @patch("solstone.observe.embed.is_available", return_value=True)
    @patch("solstone.observe.embed._get_session")
    def test_scene_cuts_always_in_output(self, mock_get, _mock_avail):
        vec = np.ones((1, EMBEDDING_DIM), dtype=np.float32)
        mock_get.return_value = _mock_session(fixed_vec=vec)
        frames = [
            _frame(0),
            _frame(1, scene_cut=True),
            _frame(2),
        ]
        result = reduce_frames(frames, distance_threshold=0.10, min_frames=1)
        ids = [f["frame_id"] for f in result]
        assert 0 in ids
        assert 1 in ids

    @patch("solstone.observe.embed.is_available", return_value=True)
    @patch("solstone.observe.embed._get_session")
    def test_min_frames_threads_through(self, mock_get, _mock_avail):
        vec = np.ones((1, EMBEDDING_DIM), dtype=np.float32)
        mock_get.return_value = _mock_session(fixed_vec=vec)
        frames = [_frame(i, timestamp=float(i * 10)) for i in range(10)]
        result = reduce_frames(frames, distance_threshold=0.10, min_frames=5)
        assert len(result) == 5
