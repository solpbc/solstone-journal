#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Deterministic VAD fixtures for the native Silero VAD v6 helper.

Three committed artefacts come out of this script:

``core/fixtures/vad_speech_seed.f32le``
    16384 raw little-endian float32 samples (65536 bytes) of synthetic voiced
    audio that Silero VAD v6 actually classifies as speech. The differential
    corpus in ``core/crates/solstone-core-vad-analyze/tests/vad_differential.rs``
    builds every one of its named cases by concatenating and repeating *these*
    bytes with zero-silence windows, so the Rust helper and the Python
    reference are handed the same buffer rather than two independent
    synthesis attempts.

``core/fixtures/vad_probability_seed.f32le``
    1024 samples (4096 bytes) of the same synthesis at a shorter period. It is
    the tile the probability oracle below is built from.

``core/fixtures/vad_probability_oracle.json``
    The frozen per-window probability sequence the reference
    ``SileroVADModel.__call__`` produced for the probability seed tiled
    ``TILE_COUNT`` times. The tiling rule is exact and the Rust side replays
    it: ``1024 * 5001 = 5_121_024`` raw samples, which
    ``get_speech_timestamps``' production pad
    (``512 - len % 512``, a *full* extra window when the length is already a
    multiple of 512) extends to ``5_121_536``, giving **10_003** encoder
    windows -- past the 10_000-window ``encoder_batch_size`` boundary, so the
    frozen sequence covers the batch seam where the LSTM state must carry from
    one encoder batch into the next.

Both seeds are periodic over their own length: every partial (``f0 * k``), the
pitch-modulation term, the formant sweep, and the syllabic envelope all
complete a whole number of cycles across the tile, so repeating a seed produces
no discontinuity at the join.

⛔ **No gate runs this script.** The artefacts are a FROZEN record: the
consumer reads the committed bytes and compares bit-for-bit. Re-running it
against a different ONNX Runtime build, a different model, or edited synthesis
parameters rewrites the oracle rather than testing against it, which would turn
a real divergence into a silent baseline update.

Recorded provenance: built 2026-08-10 on linux-x86_64 against the pinned
``silero_vad_v6.onnx`` and the ``onnxruntime`` version reported in the oracle's
``onnxruntime_version`` field.

Usage::

    .venv/bin/python3 scripts/generate_vad_fixtures.py
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "core" / "fixtures"
REFERENCE = ROOT / "solstone" / "observe" / "_silero_vad.py"
MODEL = (
    ROOT
    / "packages"
    / "solstone-journal-models"
    / "solstone_journal_models"
    / "assets"
    / "silero_vad_v6.onnx"
)

ORACLE_SCHEMA = "solstone-vad-probability-oracle-v1"
ARCHITECTURE = "linux-x86_64"

SAMPLE_RATE_HZ = 16000
WINDOW_SIZE_SAMPLES = 512
CONTEXT_SIZE_SAMPLES = 64
ENCODER_BATCH_SIZE = 10000

SPEECH_SEED_SAMPLES = 16384
PROBABILITY_SEED_SAMPLES = 1024
# 5001 * 1024 = 5_121_024 raw samples -> 10_003 windows after the production
# pad, which is past ENCODER_BATCH_SIZE.
TILE_COUNT = 5001

AMPLITUDE = 0.3
# Three resonances with their own bandwidths, swept together across the tile.
FORMANTS = ((700.0, 250.0, 120.0, 1.00, 0.0), (1220.0, 400.0, 140.0, 0.60, 1.0), (2600.0, 300.0, 160.0, 0.35, 2.0))
PITCH_MODULATION_RADIANS = 0.6


def _voiced(
    samples: int,
    partials_per_tile: int,
    syllable_cycles: int,
    pitch_cycles: int,
    formant_cycles: int,
) -> np.ndarray:
    """A synthetic voiced tile of ``samples`` float32 samples.

    A stationary tone is not enough: Silero scores the onset and then decays,
    so a repeated stationary tile reads as silence within about fifty windows.
    What keeps it reading as speech is non-stationarity -- a modulated pitch, a
    swept formant structure, and a syllabic amplitude envelope, all of which
    this adds.

    ``partials_per_tile`` fixes the fundamental at exactly that many periods
    per tile (``f0 = SAMPLE_RATE_HZ * partials_per_tile / samples``), and every
    modulator is indexed by tile fraction, so the tile is periodic and tiling
    it introduces no edge.
    """
    index = np.arange(samples, dtype=np.float64)
    fraction = index / samples
    f0 = SAMPLE_RATE_HZ * partials_per_tile / samples
    pitch = PITCH_MODULATION_RADIANS * np.sin(2.0 * np.pi * pitch_cycles * fraction)
    centers = [
        center + sweep * np.sin(2.0 * np.pi * formant_cycles * fraction + phase)
        for center, sweep, _bandwidth, _weight, phase in FORMANTS
    ]

    signal = np.zeros(samples, dtype=np.float64)
    harmonic = 1
    while f0 * harmonic < SAMPLE_RATE_HZ / 2:
        frequency = f0 * harmonic
        gain = np.zeros(samples, dtype=np.float64)
        for center, (_c, _s, bandwidth, weight, _p) in zip(centers, FORMANTS, strict=True):
            gain += weight / (1.0 + ((frequency - center) / bandwidth) ** 2)
        signal += gain * np.sin(2.0 * np.pi * frequency * index / SAMPLE_RATE_HZ + harmonic * pitch) / harmonic
        harmonic += 1

    signal *= 0.5 * (1.0 - np.cos(2.0 * np.pi * syllable_cycles * fraction))
    signal /= np.abs(signal).max()
    return (AMPLITUDE * signal).astype(np.float32)


def speech_seed() -> np.ndarray:
    """16384 samples: 123 fundamental periods, 5 syllables, per tile."""
    return _voiced(
        SPEECH_SEED_SAMPLES,
        partials_per_tile=123,
        syllable_cycles=5,
        pitch_cycles=2,
        formant_cycles=3,
    )


def probability_seed() -> np.ndarray:
    """1024 samples: 8 fundamental periods (125 Hz), one envelope cycle."""
    return _voiced(
        PROBABILITY_SEED_SAMPLES,
        partials_per_tile=8,
        syllable_cycles=1,
        pitch_cycles=2,
        formant_cycles=3,
    )


def _load_reference():
    """Loads ``_silero_vad.py`` by file path, the way the differential does."""
    spec = importlib.util.spec_from_file_location("silero_vad_reference", REFERENCE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _write(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    print(f"wrote {path.relative_to(ROOT)} ({len(payload)} bytes, sha256={_sha256(payload)})")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.parse_args(argv)

    import onnxruntime

    reference = _load_reference()

    speech = speech_seed()
    probability = probability_seed()
    assert speech.shape == (SPEECH_SEED_SAMPLES,), speech.shape
    assert probability.shape == (PROBABILITY_SEED_SAMPLES,), probability.shape

    _write(FIXTURES / "vad_speech_seed.f32le", speech.tobytes())
    seed_bytes = probability.tobytes()
    _write(FIXTURES / "vad_probability_seed.f32le", seed_bytes)

    raw = np.tile(probability, TILE_COUNT)
    padded = np.pad(raw, (0, WINDOW_SIZE_SAMPLES - raw.shape[0] % WINDOW_SIZE_SAMPLES))
    model = reference.SileroVADModel(str(MODEL))
    probabilities = model(padded)
    assert probabilities.shape == (padded.shape[0] // WINDOW_SIZE_SAMPLES,), probabilities.shape
    assert probabilities.shape[0] > ENCODER_BATCH_SIZE, "oracle must cross the encoder batch seam"

    oracle = {
        "schema": ORACLE_SCHEMA,
        "architecture": ARCHITECTURE,
        "onnxruntime_version": onnxruntime.__version__,
        "model_sha256": _sha256(MODEL.read_bytes()),
        "seed_sha256": _sha256(seed_bytes),
        "sample_rate_hz": SAMPLE_RATE_HZ,
        "window_size_samples": WINDOW_SIZE_SAMPLES,
        "context_size_samples": CONTEXT_SIZE_SAMPLES,
        "encoder_batch_size": ENCODER_BATCH_SIZE,
        "seed_samples": PROBABILITY_SEED_SAMPLES,
        "tile_count": TILE_COUNT,
        "raw_samples": int(raw.shape[0]),
        "padded_samples": int(padded.shape[0]),
        "window_count": int(probabilities.shape[0]),
        # float() on a numpy float32 widens exactly, and json writes the
        # shortest decimal that round-trips that double -- so a reader parses
        # back the identical double and narrows to the identical float32.
        "speech_probabilities": [float(value) for value in probabilities],
    }
    oracle_path = FIXTURES / "vad_probability_oracle.json"
    _write(oracle_path, (json.dumps(oracle, indent=2) + "\n").encode("utf-8"))
    print(
        f"oracle: {oracle['window_count']} windows from {oracle['tile_count']} tiles "
        f"({oracle['raw_samples']} raw -> {oracle['padded_samples']} padded samples), "
        f"onnxruntime {oracle['onnxruntime_version']}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
