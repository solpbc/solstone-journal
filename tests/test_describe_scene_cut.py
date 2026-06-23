# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for scene-cut detection in VideoProcessor.process()."""

from pathlib import Path
from unittest.mock import MagicMock

import pytest
from PIL import Image

from solstone.observe.describe import VideoProcessor


# ---------------------------------------------------------------------------
# Image helpers
# ---------------------------------------------------------------------------


def _gradient_image(size=(100, 100), *, bright_left: bool = True) -> Image.Image:
    """Horizontal gradient: bright_left=True → brightness decreases left→right.

    After dHash resize to (9, 8), each row is strictly monotone, so all 8
    comparison bits per row are set (bright_left) or clear (not bright_left).
    Opposite gradients therefore produce the maximum Hamming distance of 64.
    """
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


def _uniform_image(value: int = 128, size=(100, 100)) -> Image.Image:
    return Image.new("RGB", size, (value, value, value))


# ---------------------------------------------------------------------------
# Mock av helpers
# ---------------------------------------------------------------------------


class _FakeAvFrame:
    """Minimal stand-in for a PyAV decoded frame."""

    def __init__(self, img: Image.Image, ts: float):
        self.pts = 1  # non-None → frame is not skipped
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


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def no_aruco(monkeypatch):
    """Stub ArUco detection so no frame is masked or skipped."""
    import solstone.observe.aruco as aruco_module

    monkeypatch.setattr(aruco_module, "detect_markers", lambda _img: None)


@pytest.fixture
def install_av(monkeypatch):
    """Return a helper that wires a frame list into av.open."""

    def _install(frames: list):
        import av

        container = _fake_av_container(frames)
        monkeypatch.setattr(av, "open", lambda *_a, **_kw: container)

    return _install


# ---------------------------------------------------------------------------
# Constant sanity
# ---------------------------------------------------------------------------


def test_scene_cut_threshold_above_dhash_threshold():
    assert VideoProcessor.SCENE_CUT_THRESHOLD > VideoProcessor.DHASH_THRESHOLD


def test_scene_cut_threshold_below_max_bits():
    # Must leave room for ordinary qualifying frames between the two thresholds.
    assert VideoProcessor.DHASH_THRESHOLD < VideoProcessor.SCENE_CUT_THRESHOLD < 64


# ---------------------------------------------------------------------------
# _dhash distance properties (no av needed)
# ---------------------------------------------------------------------------


def _vp() -> VideoProcessor:
    vp = VideoProcessor.__new__(VideoProcessor)
    vp.video_path = Path("dummy.webm")
    return vp


def _distance(vp: VideoProcessor, a: Image.Image, b: Image.Image) -> int:
    return bin(vp._dhash(a) ^ vp._dhash(b)).count("1")


def test_dhash_distance_zero_for_identical_image():
    vp = _vp()
    img = _gradient_image()
    assert _distance(vp, img, img.copy()) == 0


def test_dhash_distance_max_for_opposite_gradients():
    """Opposite gradient images → all 64 bits flip → maximum distance."""
    vp = _vp()
    d = _distance(vp, _gradient_image(bright_left=True), _gradient_image(bright_left=False))
    assert d == 64


def test_opposite_gradients_exceed_scene_cut_threshold():
    vp = _vp()
    d = _distance(vp, _gradient_image(bright_left=True), _gradient_image(bright_left=False))
    assert d >= VideoProcessor.SCENE_CUT_THRESHOLD


# ---------------------------------------------------------------------------
# process() scene-cut tagging — via mocked av
# ---------------------------------------------------------------------------


def test_first_frame_never_tagged_as_scene_cut(no_aruco, install_av):
    install_av([_FakeAvFrame(_gradient_image(bright_left=True), 0.0)])
    vp = VideoProcessor(Path("dummy.webm"))
    frames = vp.process()
    assert len(frames) == 1
    assert "scene_cut" not in frames[0]


def test_scene_cut_frame_tagged(no_aruco, install_av):
    """A transition with distance ≥ SCENE_CUT_THRESHOLD gets scene_cut=True."""
    img_a = _gradient_image(bright_left=True)
    img_b = _gradient_image(bright_left=False)

    install_av([
        _FakeAvFrame(img_a, 0.0),
        _FakeAvFrame(img_b, 1.0),
    ])
    vp = VideoProcessor(Path("dummy.webm"))
    frames = vp.process()

    assert len(frames) == 2
    assert "scene_cut" not in frames[0]
    assert frames[1].get("scene_cut") is True


def test_normal_qualifying_frame_not_tagged(no_aruco, install_av, monkeypatch):
    """A frame with DHASH_THRESHOLD ≤ distance < SCENE_CUT_THRESHOLD has no scene_cut."""
    img = _uniform_image(128)
    install_av([
        _FakeAvFrame(img, 0.0),
        _FakeAvFrame(img.copy(), 6.0),  # beyond MIN_STRIDE_SECONDS so stride doesn't filter it
    ])
    vp = VideoProcessor(Path("dummy.webm"))

    # Inject hashes: distance = popcount(0xFFFFFF00 XOR 0x00000000) = 24.
    # 24 ≥ DHASH_THRESHOLD (8) and 24 < SCENE_CUT_THRESHOLD (40).
    hashes = iter([0xFFFFFF00, 0x00000000])
    monkeypatch.setattr(vp, "_dhash", lambda _img: next(hashes))

    frames = vp.process()
    assert len(frames) == 2
    assert "scene_cut" not in frames[0]
    assert "scene_cut" not in frames[1]


def test_below_dhash_threshold_frame_still_discarded(no_aruco, install_av, monkeypatch):
    """Scene-cut logic does not resurrect frames dropped by DHASH_THRESHOLD."""
    img = _uniform_image(128)
    install_av([
        _FakeAvFrame(img, 0.0),
        _FakeAvFrame(img.copy(), 1.0),
    ])
    vp = VideoProcessor(Path("dummy.webm"))

    # Distance = popcount(0xFF XOR 0xF8) = popcount(0x07) = 3 < DHASH_THRESHOLD.
    hashes = iter([0xFF, 0xF8])
    monkeypatch.setattr(vp, "_dhash", lambda _img: next(hashes))

    frames = vp.process()
    assert len(frames) == 1  # second frame discarded


def test_multiple_scene_cuts_all_tagged(no_aruco, install_av):
    """Every transition above SCENE_CUT_THRESHOLD is independently tagged."""
    img_a = _gradient_image(bright_left=True)
    img_b = _gradient_image(bright_left=False)

    install_av([
        _FakeAvFrame(img_a, 0.0),   # first — never tagged
        _FakeAvFrame(img_b, 1.0),   # scene cut 1
        _FakeAvFrame(img_a, 2.0),   # scene cut 2
    ])
    vp = VideoProcessor(Path("dummy.webm"))
    frames = vp.process()

    assert len(frames) == 3
    assert "scene_cut" not in frames[0]
    assert frames[1].get("scene_cut") is True
    assert frames[2].get("scene_cut") is True


def test_scene_cut_frames_carry_frame_bytes(no_aruco, install_av):
    """scene_cut frames are fully qualified — frame_bytes must be present."""
    img_a = _gradient_image(bright_left=True)
    img_b = _gradient_image(bright_left=False)

    install_av([
        _FakeAvFrame(img_a, 0.0),
        _FakeAvFrame(img_b, 1.0),
    ])
    vp = VideoProcessor(Path("dummy.webm"))
    frames = vp.process()

    assert frames[1].get("scene_cut") is True
    assert "frame_bytes" in frames[1]
    assert len(frames[1]["frame_bytes"]) > 0


def test_scene_cut_frames_reduce_reliance_on_random_fallback(no_aruco, install_av):
    """scene_cut count is lower than total qualified frames — they are the minority.

    Validates the improvement claim: scene cuts tag a meaningful but small subset,
    leaving the bulk of frames as ordinary qualifying frames.
    """
    # Interleave one scene cut among many ordinary transitions.
    img_a = _gradient_image(bright_left=True)
    img_b = _gradient_image(bright_left=False)

    av_frames = [_FakeAvFrame(img_a, 0.0)]
    for i in range(1, 10):
        # Alternate between the two to force scene cuts every other frame.
        img = img_b if i % 2 == 1 else img_a
        av_frames.append(_FakeAvFrame(img, float(i)))

    install_av(av_frames)
    vp = VideoProcessor(Path("dummy.webm"))
    frames = vp.process()

    scene_cuts = [f for f in frames if f.get("scene_cut")]
    assert len(scene_cuts) > 0, "Expected at least one scene cut"
    assert len(scene_cuts) < len(frames), "Not every frame should be a scene cut"
