# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the stride floor in VideoProcessor.process()."""

from pathlib import Path
from unittest.mock import MagicMock

import pytest
from PIL import Image

from solstone.observe.describe import VideoProcessor

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _uniform_image(value: int = 128, size=(100, 100)) -> Image.Image:
    return Image.new("RGB", size, (value, value, value))


def _gradient_image(size=(100, 100), *, bright_left: bool = True) -> Image.Image:
    img = Image.new("RGB", size)
    w, h = size
    pixels = []
    for _ in range(h):
        for col in range(w):
            val = int(255 * col / max(w - 1, 1))
            v = (255 - val) if bright_left else val
            pixels.append((v, v, v))
    img.putdata(pixels)
    return img


class _FakeAvFrame:
    def __init__(self, img: Image.Image, ts: float):
        self.pts = 1
        self.time = ts
        self._img = img.convert("RGB")

    def to_ndarray(self, format=None):  # noqa: A002
        import numpy as np

        return np.array(self._img)


def _fake_av_container(frames: list) -> MagicMock:
    stream = MagicMock()
    stream.width = 100
    stream.height = 100
    container = MagicMock()
    container.streams.video = [stream]
    container.decode.return_value = iter(frames)
    container.__enter__ = MagicMock(return_value=container)
    container.__exit__ = MagicMock(return_value=False)
    return container


def _install_av(monkeypatch, frames):
    import av

    monkeypatch.setattr(av, "open", lambda *_a, **_kw: _fake_av_container(frames))


def _install_hashes(monkeypatch, vp, hashes):
    """Inject a fixed hash sequence into vp._dhash."""
    it = iter(hashes)
    monkeypatch.setattr(vp, "_dhash", lambda _img: next(it))


@pytest.fixture
def no_aruco(monkeypatch):
    import solstone.observe.aruco as aruco_module

    monkeypatch.setattr(aruco_module, "detect_markers", lambda _img: None)


# ---------------------------------------------------------------------------
# Constant sanity
# ---------------------------------------------------------------------------


def test_min_stride_seconds_is_positive():
    assert VideoProcessor.MIN_STRIDE_SECONDS > 0


def test_min_stride_seconds_is_five():
    assert VideoProcessor.MIN_STRIDE_SECONDS == 5.0


def test_scene_cut_threshold_still_above_dhash_threshold():
    # Stride floor must not affect the scene-cut/dHash relationship.
    assert VideoProcessor.DHASH_THRESHOLD < VideoProcessor.SCENE_CUT_THRESHOLD


# ---------------------------------------------------------------------------
# Stride floor behaviour
# ---------------------------------------------------------------------------


def test_first_frame_always_kept_regardless_of_stride(no_aruco, monkeypatch):
    _install_av(monkeypatch, [_FakeAvFrame(_uniform_image(), 0.0)])
    assert len(VideoProcessor(Path("dummy.webm")).process()) == 1


def test_frame_within_stride_window_is_dropped(no_aruco, monkeypatch):
    """A qualifying frame arriving before MIN_STRIDE_SECONDS is discarded."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),
            _FakeAvFrame(img.copy(), 3.0),  # 3s gap < 5s
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    # distance 24: passes dHash, not a scene cut
    _install_hashes(monkeypatch, vp, [0xFFFFFF00, 0x00000000])
    assert len(vp.process()) == 1


def test_frame_at_exactly_stride_boundary_is_kept(no_aruco, monkeypatch):
    """A frame at exactly MIN_STRIDE_SECONDS is kept (>= not >)."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),
            _FakeAvFrame(img.copy(), 5.0),  # exactly 5s
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    _install_hashes(monkeypatch, vp, [0xFFFFFF00, 0x00000000])
    assert len(vp.process()) == 2


def test_frame_beyond_stride_window_is_kept(no_aruco, monkeypatch):
    """A qualifying frame arriving after MIN_STRIDE_SECONDS is kept."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),
            _FakeAvFrame(img.copy(), 6.0),  # 6s gap > 5s
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    _install_hashes(monkeypatch, vp, [0xFFFFFF00, 0x00000000])
    assert len(vp.process()) == 2


def test_stride_clock_resets_after_kept_frame(no_aruco, monkeypatch):
    """Stride window is measured from the last KEPT frame, not the first frame."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),  # first — kept, clock=0.0
            _FakeAvFrame(img, 6.0),  # 6s gap — kept, clock resets to 6.0
            _FakeAvFrame(img, 9.0),  # 3s after t=6.0 — dropped
            _FakeAvFrame(img, 12.0),  # 6s after t=6.0 — kept
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    # All pairs: distance 24 (qualifies, not scene cut).
    # Sequence: h0=0xFFFFFF00, h1=0x00000000 (kept), h2=0xFFFFFF00 (filtered,
    # compared against h1), h3=0xFFFFFF00 (kept, compared against h1 since h2 filtered).
    _install_hashes(monkeypatch, vp, [0xFFFFFF00, 0x00000000, 0xFFFFFF00, 0xFFFFFF00])
    frames = vp.process()
    assert [f["timestamp"] for f in frames] == [0.0, 6.0, 12.0]


def test_scene_cut_bypasses_stride_floor(no_aruco, monkeypatch):
    """A scene cut is always kept even if it arrives within the stride window."""
    img_a = _gradient_image(bright_left=True)
    img_b = _gradient_image(bright_left=False)
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img_a, 0.0),
            _FakeAvFrame(
                img_b, 1.0
            ),  # 1s gap — would be filtered, but distance=64 → scene cut
        ],
    )
    frames = VideoProcessor(Path("dummy.webm")).process()
    assert len(frames) == 2
    assert frames[1].get("scene_cut") is True


def test_stride_does_not_update_hash_for_filtered_frames(no_aruco, monkeypatch):
    """last_hash stays at the last KEPT frame — filtered frames don't shift it."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),  # kept, last_hash=h0
            _FakeAvFrame(img, 3.0),  # stride-filtered, h1 consumed, last_hash stays h0
            _FakeAvFrame(img, 11.0),  # 11s from t=0.0, compared against h0 → kept
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    # h0=0xFFFFFF00, h1=0x00000000 (filtered), h2=0xFF000000
    # dist(h0, h1)=24 → filtered; dist(h0, h2)=16 → kept (compared against h0 not h1)
    _install_hashes(monkeypatch, vp, [0xFFFFFF00, 0x00000000, 0xFF000000])
    frames = vp.process()
    assert [f["timestamp"] for f in frames] == [0.0, 11.0]


def test_multiple_stride_filtered_frames_in_sequence(no_aruco, monkeypatch):
    """Several consecutive qualifying frames within the stride window are all dropped."""
    img = _uniform_image()
    _install_av(
        monkeypatch,
        [
            _FakeAvFrame(img, 0.0),  # kept
            _FakeAvFrame(img, 1.0),  # filtered (1s)
            _FakeAvFrame(img, 2.0),  # filtered (2s)
            _FakeAvFrame(img, 3.0),  # filtered (3s)
            _FakeAvFrame(img, 4.0),  # filtered (4s)
            _FakeAvFrame(img, 6.0),  # kept (6s > 5s)
        ],
    )
    vp = VideoProcessor(Path("dummy.webm"))
    # h0 kept; h1-h5 all distance 24 from h0 (and from each other since last_hash
    # stays h0 throughout — all filtered frames don't update last_hash).
    _install_hashes(
        monkeypatch,
        vp,
        [0xFFFFFF00, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000],
    )
    frames = vp.process()
    assert len(frames) == 2
    assert frames[1]["timestamp"] == 6.0
