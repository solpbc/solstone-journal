# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the vendored Silero VAD module."""

import subprocess
import sys

import pytest

PROBE = """
import sys
import solstone.observe.vad  # noqa: F401
import solstone.observe.transcribe  # noqa: F401
assert "faster_whisper" not in sys.modules, sorted(m for m in sys.modules if "faster" in m)
sys.stdout.write("ok\\n")
"""


def test_vendored_module_does_not_pull_faster_whisper():
    """This test would fail on pre-L2 main because VAD loaded faster_whisper.vad at module load time."""
    result = subprocess.run(
        [sys.executable, "-c", PROBE],
        capture_output=True,
        text=True,
        check=True,
    )

    assert "ok" in result.stdout


def test_vendored_get_speech_timestamps_matches_upstream():
    """Vendored VAD should return the same speech segment bounds as upstream."""
    upstream_vad = pytest.importorskip("faster_whisper.vad")

    from solstone.observe import _silero_vad as vendored_vad
    from solstone.observe.utils import SAMPLE_RATE, load_audio
    from solstone.think.install_models import _fixture_audio_path

    audio = load_audio(_fixture_audio_path())
    vendored_segments = vendored_vad.get_speech_timestamps(
        audio,
        vendored_vad.VadOptions(),
        sampling_rate=SAMPLE_RATE,
    )
    upstream_segments = upstream_vad.get_speech_timestamps(
        audio,
        upstream_vad.VadOptions(),
        sampling_rate=SAMPLE_RATE,
    )

    assert isinstance(vendored_segments, list)
    assert isinstance(upstream_segments, list)
    assert len(vendored_segments) == len(upstream_segments)
    for vendored, upstream in zip(vendored_segments, upstream_segments):
        assert vendored["start"] == upstream["start"]
        assert vendored["end"] == upstream["end"]


def test_vendored_get_vad_model_loads_asset():
    """Vendored VAD should load the bundled ONNX asset."""
    from solstone.observe._silero_vad import get_vad_model

    assert get_vad_model() is not None
