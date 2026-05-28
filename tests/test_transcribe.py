# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observe.transcribe module."""

import json
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path

import av
import numpy as np
import pytest
import soundfile as sf

from solstone.observe.transcribe import (
    DEFAULT_COMPUTE,
    DEFAULT_DEVICE,
    DEFAULT_MIN_SPEECH_SECONDS,
    DEFAULT_MODEL,
    MIN_STATEMENT_DURATION,
    SENTENCE_ENDINGS,
    build_statement,
    build_statements_from_acoustic,
)
from solstone.observe.transcribe.main import EMBEDDER_NAME, _statements_to_jsonl
from solstone.observe.utils import load_audio
from solstone.observe.vad import VadResult
from solstone.think.media import AUDIO_EXTENSIONS

LOAD_AUDIO_PROBE = """
import sys
from pathlib import Path

from solstone.observe.utils import load_audio
from solstone.think.media import AUDIO_EXTENSIONS

for suffix in sorted(AUDIO_EXTENSIONS):
    try:
        load_audio(Path("/tmp/solstone-load-audio-probe").with_suffix(suffix))
    except Exception:
        pass
assert "faster_whisper" not in sys.modules, sorted(m for m in sys.modules if "faster" in m)
sys.stdout.write("PROBE_OK\\n")
"""


class TestBuildStatementsFromAcoustic:
    """Test building statements from acoustic segments."""

    def test_merges_fragments_into_statement(self):
        """Multiple acoustic segments forming one sentence should merge."""
        # Simulates Whisper splitting "I think I can do it." across 3 acoustic segments
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 1.0,
                "text": "I think",
                "words": [
                    {"word": " I", "start": 0.0, "end": 0.3, "probability": 0.9},
                    {"word": " think", "start": 0.3, "end": 1.0, "probability": 0.9},
                ],
            },
            {
                "id": 2,
                "start": 1.5,
                "end": 2.5,
                "text": "I can",
                "words": [
                    {"word": " I", "start": 1.5, "end": 1.8, "probability": 0.9},
                    {"word": " can", "start": 1.8, "end": 2.5, "probability": 0.9},
                ],
            },
            {
                "id": 3,
                "start": 3.0,
                "end": 4.0,
                "text": "do it.",
                "words": [
                    {"word": " do", "start": 3.0, "end": 3.3, "probability": 0.9},
                    {"word": " it.", "start": 3.3, "end": 4.0, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        assert len(result) == 1
        stmt = result[0]
        assert stmt["id"] == 1
        assert stmt["start"] == 0.0
        assert stmt["end"] == 4.0
        assert stmt["text"] == "I think I can do it."
        assert len(stmt["words"]) == 6

    def test_splits_on_period(self):
        """Statements should split on period."""
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 5.0,
                "text": "Hello. World.",
                "words": [
                    {"word": " Hello.", "start": 0.0, "end": 1.0, "probability": 0.9},
                    {"word": " World.", "start": 2.0, "end": 3.0, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        assert len(result) == 2
        assert result[0]["text"] == "Hello."
        assert result[1]["text"] == "World."

    def test_splits_on_question_mark(self):
        """Statements should split on question mark."""
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 3.0,
                "text": "How are you? Good.",
                "words": [
                    {"word": " How", "start": 0.0, "end": 0.3, "probability": 0.9},
                    {"word": " are", "start": 0.3, "end": 0.6, "probability": 0.9},
                    {"word": " you?", "start": 0.6, "end": 1.0, "probability": 0.9},
                    {"word": " Good.", "start": 2.0, "end": 3.0, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        assert len(result) == 2
        assert result[0]["text"] == "How are you?"
        assert result[1]["text"] == "Good."

    def test_splits_on_exclamation(self):
        """Statements should split on exclamation mark."""
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 2.0,
                "text": "Wow! Amazing.",
                "words": [
                    {"word": " Wow!", "start": 0.0, "end": 0.5, "probability": 0.9},
                    {"word": " Amazing.", "start": 1.0, "end": 2.0, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        assert len(result) == 2
        assert result[0]["text"] == "Wow!"
        assert result[1]["text"] == "Amazing."

    def test_handles_incomplete_final_sentence(self):
        """Final sentence without punctuation should still be captured."""
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 3.0,
                "text": "First sentence. And then",
                "words": [
                    {"word": " First", "start": 0.0, "end": 0.3, "probability": 0.9},
                    {
                        "word": " sentence.",
                        "start": 0.3,
                        "end": 1.0,
                        "probability": 0.9,
                    },
                    {"word": " And", "start": 1.5, "end": 1.8, "probability": 0.9},
                    {"word": " then", "start": 1.8, "end": 2.0, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        assert len(result) == 2
        assert result[0]["text"] == "First sentence."
        assert result[1]["text"] == "And then"

    def test_empty_segments_returns_unchanged(self):
        """Empty acoustic segments should return unchanged."""
        acoustic_segments = []
        result = build_statements_from_acoustic(acoustic_segments)
        assert result == acoustic_segments

    def test_statement_timestamps_from_words(self):
        """Statement start/end should come from first/last word."""
        acoustic_segments = [
            {
                "id": 1,
                "start": 0.0,
                "end": 10.0,  # Original segment end
                "text": "Hello world.",
                "words": [
                    {"word": " Hello", "start": 2.5, "end": 3.0, "probability": 0.9},
                    {"word": " world.", "start": 3.5, "end": 4.2, "probability": 0.9},
                ],
            },
        ]

        result = build_statements_from_acoustic(acoustic_segments)

        stmt = result[0]
        assert stmt["start"] == 2.5  # From first word
        assert stmt["end"] == 4.2  # From last word


class TestBuildStatement:
    """Test statement building helper."""

    def test_builds_statement_from_words(self):
        """Should build statement with correct fields."""
        words = [
            {"word": " Hello", "start": 0.0, "end": 0.5, "probability": 0.9},
            {"word": " world", "start": 0.6, "end": 1.0, "probability": 0.8},
        ]

        stmt = build_statement(1, words)

        assert stmt["id"] == 1
        assert stmt["start"] == 0.0
        assert stmt["end"] == 1.0
        assert stmt["text"] == "Hello world"
        assert stmt["words"] == words


class TestConstants:
    """Test module constants."""

    def test_sentence_endings(self):
        """SENTENCE_ENDINGS should contain expected punctuation."""
        assert "." in SENTENCE_ENDINGS
        assert "?" in SENTENCE_ENDINGS
        assert "!" in SENTENCE_ENDINGS
        assert "," not in SENTENCE_ENDINGS

    def test_min_statement_duration(self):
        """MIN_STATEMENT_DURATION should be positive."""
        assert MIN_STATEMENT_DURATION > 0

    def test_default_transcription_settings(self):
        """Default transcription settings should be valid."""
        assert DEFAULT_MODEL == "medium.en"
        assert DEFAULT_DEVICE == "auto"
        assert DEFAULT_COMPUTE == "default"
        assert DEFAULT_MIN_SPEECH_SECONDS == 1.0


class TestLoadAudio:
    """Test the shared load_audio utility."""

    def test_load_audio_does_not_pull_faster_whisper(self):
        result = subprocess.run(
            [sys.executable, "-c", LOAD_AUDIO_PROBE],
            capture_output=True,
            text=True,
            check=True,
        )

        assert "PROBE_OK" in result.stdout

    def test_flac_returns_numpy_array(self):
        """FLAC files should return a numpy array."""
        with tempfile.TemporaryDirectory() as tmpdir:
            flac_path = Path(tmpdir) / "test.flac"

            # Create a simple FLAC file
            sample_rate = 16000
            data = np.zeros(sample_rate, dtype=np.float32)
            sf.write(flac_path, data, sample_rate, format="FLAC")

            result = load_audio(flac_path)
            assert isinstance(result, np.ndarray)
            assert result.dtype == np.float32
            assert len(result) == sample_rate

    @pytest.mark.skipif(not shutil.which("ffmpeg"), reason="ffmpeg not installed")
    def test_m4a_returns_numpy_array(self):
        """M4A files should return a numpy array with audio content."""
        import subprocess

        with tempfile.TemporaryDirectory() as tmpdir:
            # Create source FLAC
            flac_path = Path(tmpdir) / "source.flac"
            sample_rate = 16000
            duration = 0.5
            t = np.linspace(0, duration, int(sample_rate * duration), dtype=np.float32)
            data = 0.5 * np.sin(2 * np.pi * 440 * t)
            sf.write(flac_path, data, sample_rate, format="FLAC")

            # Convert to M4A
            m4a_path = Path(tmpdir) / "test.m4a"
            result = subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-i",
                    str(flac_path),
                    "-c:a",
                    "aac",
                    "-b:a",
                    "64k",
                    str(m4a_path),
                ],
                capture_output=True,
            )
            assert result.returncode == 0

            # Test loading returns numpy array
            audio = load_audio(m4a_path)
            assert isinstance(audio, np.ndarray)
            assert audio.dtype == np.float32
            assert len(audio) > 0

    @pytest.mark.skipif(not shutil.which("ffmpeg"), reason="ffmpeg not installed")
    def test_multi_track_m4a_mixes_streams(self):
        """load_audio should mix multiple M4A audio streams together."""
        import subprocess

        with tempfile.TemporaryDirectory() as tmpdir:
            # Create two mono FLAC files to combine into multi-track M4A
            track0_path = Path(tmpdir) / "track0.flac"
            track1_path = Path(tmpdir) / "track1.flac"
            m4a_path = Path(tmpdir) / "test.m4a"

            # Track 0: silence (system audio - no content)
            # Track 1: 440Hz sine wave (microphone - has voice)
            sample_rate = 16000
            duration = 1.0  # 1 second
            t = np.linspace(0, duration, int(sample_rate * duration), dtype=np.float32)

            track0_data = np.zeros_like(t)  # Silence
            track1_data = 0.5 * np.sin(2 * np.pi * 440 * t)  # 440Hz tone

            sf.write(track0_path, track0_data, sample_rate, format="FLAC")
            sf.write(track1_path, track1_data, sample_rate, format="FLAC")

            # Use ffmpeg to create multi-track M4A
            result = subprocess.run(
                [
                    "ffmpeg",
                    "-y",
                    "-i",
                    str(track0_path),
                    "-i",
                    str(track1_path),
                    "-map",
                    "0:a",
                    "-map",
                    "1:a",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "64k",
                    str(m4a_path),
                ],
                capture_output=True,
                text=True,
            )
            assert result.returncode == 0, f"ffmpeg failed: {result.stderr}"

            audio = load_audio(m4a_path)

            assert isinstance(audio, np.ndarray)
            assert audio.dtype == np.float32

            # The mixed audio should have content from track 1 (the sine wave)
            # AAC compression affects amplitude, so use loose threshold
            rms = np.sqrt(np.mean(audio**2))
            assert rms > 0.1, f"Mixed audio should contain signal, got RMS={rms}"

    @pytest.mark.parametrize("suffix", sorted(AUDIO_EXTENSIONS - {".m4a"}))
    def test_load_audio_decodes_ext(self, tmp_path, suffix):
        sample_rate = 48000
        duration = 1.0
        t = np.arange(int(sample_rate * duration), dtype=np.float32) / sample_rate
        data = np.sin(2 * np.pi * 440 * t).astype(np.float32)
        path = tmp_path / f"test{suffix}"

        try:
            sf.write(path, data, sample_rate)
        except Exception as e:
            pytest.skip(f"libsndfile cannot encode {suffix}: {e}")

        audio = load_audio(path)

        assert isinstance(audio, np.ndarray)
        assert audio.dtype == np.float32
        assert audio.ndim == 1
        assert abs(len(audio) - 16000) <= 64

    def test_load_audio_sine_wave_resamples_correctly(self, tmp_path):
        input_rate = 48000
        output_rate = 16000
        t = np.arange(input_rate, dtype=np.float32) / input_rate
        data = np.sin(2 * np.pi * 440 * t).astype(np.float32)
        path = tmp_path / "test.wav"
        sf.write(path, data, input_rate, format="WAV", subtype="FLOAT")

        audio = load_audio(path, sample_rate=output_rate)
        reference = np.sin(
            2 * np.pi * 440 * np.arange(output_rate, dtype=np.float32) / output_rate
        ).astype(np.float32)

        errors = []
        for shift in range(-16, 17):
            if shift >= 0:
                actual = audio[shift:]
                expected = reference
            else:
                actual = audio
                expected = reference[-shift:]
            length = min(len(actual), len(expected))
            if length <= 200:
                continue
            actual_window = actual[:length][100:-100]
            expected_window = expected[:length][100:-100]
            errors.append(float(np.max(np.abs(actual_window - expected_window))))

        assert min(errors) <= 1e-2

    def test_load_audio_wraps_decode_failure(self, tmp_path):
        path = tmp_path / "not-audio.wav"
        path.write_bytes(b"not audio")

        with pytest.raises(RuntimeError) as excinfo:
            load_audio(path)

        message = str(excinfo.value)
        assert str(path) in message
        assert "(.wav)" in message
        assert excinfo.value.__cause__ is not None
        assert isinstance(excinfo.value.__cause__, av.error.FFmpegError)

    def test_load_audio_rejects_empty_decode(self, tmp_path):
        path = tmp_path / "not-audio.flac"
        path.write_bytes(b"not audio")

        with pytest.raises(RuntimeError) as excinfo:
            load_audio(path)

        message = str(excinfo.value)
        assert str(path) in message
        assert "(.flac)" in message
        assert "no audio data decoded" in message
        assert excinfo.value.__cause__ is None

    def test_load_audio_handles_very_short_clip(self, tmp_path):
        input_rate = 48000
        output_rate = 16000
        duration = 0.05
        t = np.arange(int(input_rate * duration), dtype=np.float32) / input_rate
        data = np.sin(2 * np.pi * 440 * t).astype(np.float32)
        path = tmp_path / "short.wav"
        sf.write(path, data, input_rate, format="WAV", subtype="FLOAT")

        audio = load_audio(path, sample_rate=output_rate)

        assert audio.dtype == np.float32
        assert len(audio) > 0
        assert abs(len(audio) - 800) <= 16


class TestEmbeddingsFormat:
    """Test embeddings.npz format validation."""

    def test_embeddings_arrays_shape(self):
        """Embeddings should have correct array shapes."""
        # Simulate 10 statements with 256-dim embeddings
        embeddings = np.random.randn(10, 256).astype(np.float32)
        statement_ids = np.array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], dtype=np.int32)

        assert embeddings.shape == (10, 256)
        assert statement_ids.shape == (10,)
        assert embeddings.dtype == np.float32
        assert statement_ids.dtype == np.int32

    def test_embeddings_npz_roundtrip(self):
        """Embeddings should survive save/load cycle."""
        with tempfile.TemporaryDirectory() as tmpdir:
            npz_path = Path(tmpdir) / "embeddings.npz"

            embeddings = np.random.randn(5, 256).astype(np.float32)
            statement_ids = np.array([1, 2, 3, 4, 5], dtype=np.int32)
            encoder = np.array(EMBEDDER_NAME)

            np.savez_compressed(
                npz_path,
                embeddings=embeddings,
                statement_ids=statement_ids,
                encoder=encoder,
            )

            loaded = np.load(npz_path)
            np.testing.assert_array_almost_equal(loaded["embeddings"], embeddings)
            np.testing.assert_array_equal(loaded["statement_ids"], statement_ids)
            assert loaded["encoder"].item() == EMBEDDER_NAME

    def test_statement_ids_are_unique(self):
        """Statement IDs should be unique."""
        statement_ids = np.array([1, 2, 3, 4, 5], dtype=np.int32)
        assert len(statement_ids) == len(np.unique(statement_ids))


class TestJSONLFormat:
    """Test JSONL output format."""

    def test_statements_to_jsonl_includes_duration(self):
        """Audio metadata should include decode-derived duration."""
        lines = _statements_to_jsonl(
            [{"start": 1.0, "end": 2.0, "text": "Hello"}],
            "audio.m4a",
            datetime(2026, 5, 22, 9, 0, 0),
            {"model": "unit", "device": "cpu", "compute_type": "int8"},
            vad_result=VadResult(
                duration=12.34,
                speech_duration=1.0,
                has_speech=True,
            ),
        )

        metadata = json.loads(lines[0])

        assert metadata["duration"] == 12.34
        assert isinstance(metadata["duration"], float)

    def test_metadata_first_line(self):
        """First line should be metadata with 'raw' field."""
        lines = [
            json.dumps({"raw": "audio.flac"}),
            json.dumps({"start": "00:00:01", "text": "Hello"}),
        ]
        jsonl_content = "\n".join(lines) + "\n"

        parsed_lines = jsonl_content.strip().split("\n")
        assert len(parsed_lines) == 2

        metadata = json.loads(parsed_lines[0])
        assert "raw" in metadata
        assert metadata["raw"] == "audio.flac"

    def test_metadata_includes_transcription_config(self):
        """Metadata should include model, device, and compute_type fields."""
        # Example metadata as produced by _statements_to_jsonl()
        metadata = {
            "raw": "audio.flac",
            "model": "medium.en",
            "device": "cuda",
            "compute_type": "float16",
        }

        # Verify all config fields are present
        assert "model" in metadata
        assert "device" in metadata
        assert "compute_type" in metadata

        # Verify they have expected types
        assert isinstance(metadata["model"], str)
        assert isinstance(metadata["device"], str)
        assert isinstance(metadata["compute_type"], str)

    def test_entry_has_required_fields(self):
        """Transcript entries should have start and text."""
        entry = {"start": "00:00:01", "text": "Hello world"}

        assert "start" in entry
        assert "text" in entry

    def test_entry_source_is_optional(self):
        """Source field should be optional."""
        entry_with_source = {"start": "00:00:01", "text": "Hello", "source": "mic"}
        entry_without_source = {"start": "00:00:01", "text": "Hello"}

        # Both should be valid
        assert "text" in entry_with_source
        assert "text" in entry_without_source

    def test_speaker_not_required(self):
        """Speaker field is no longer required (no diarization)."""
        entry = {"start": "00:00:01", "text": "Hello world"}

        # Should be valid without speaker
        assert "start" in entry
        assert "text" in entry
        assert "speaker" not in entry
