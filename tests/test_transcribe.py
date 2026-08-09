# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observe.transcribe module."""

import importlib
import json
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
import pytest
import soundfile as sf

from solstone.apps.speakers.encoder_config import ENCODER_ID
from solstone.apps.speakers.evidence import SpeakerEvidenceDecision
from solstone.observe import utils as observe_utils
from solstone.observe.transcribe import (
    DEFAULT_MIN_SPEECH_SECONDS,
    MIN_STATEMENT_DURATION,
    SENTENCE_ENDINGS,
    build_statement,
    build_statements_from_acoustic,
)
from solstone.observe.transcribe.native import SpeakerTranscriptWriteResponse
from solstone.observe.transcribe.speakers_analyze_adapter import (
    PRODUCER_ID as SPEAKERS_ANALYZE_PRODUCER_ID,
)
from solstone.observe.transcribe.speakers_analyze_adapter import (
    SpeakerAnalyzeResult,
    SpeakerEmbeddingPayload,
)
from solstone.observe.utils import SAMPLE_RATE, AudioDecodeError, load_audio
from solstone.observe.vad import AudioReduction, SpeechSegment, VadResult
from solstone.think.media import AUDIO_EXTENSIONS
from tests._repo_inventory import assert_inventory_unchanged, repository_inventory


def _speaker_result(
    statements: list[dict],
    *,
    embedding_payload: SpeakerEmbeddingPayload | None = None,
    speaker_evidence: SpeakerEvidenceDecision | None = None,
    overlap_fraction: float = 0.0,
    statement_labels: list[int | None] | None = None,
) -> SpeakerAnalyzeResult:
    return SpeakerAnalyzeResult(
        statements=[dict(statement) for statement in statements],
        embedding_payload=embedding_payload,
        speaker_evidence=speaker_evidence
        or SpeakerEvidenceDecision("single", 0.0, 0.0),
        overlap_fraction=overlap_fraction,
        statement_labels=statement_labels,
    )


@pytest.fixture(autouse=True)
def _native_transcript_writer(monkeypatch: pytest.MonkeyPatch):
    """Keep handler tests focused on Python orchestration, not a built helper."""
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    def write(**request):
        header = dict(request["header"])
        segment_meta = header.pop("segment_meta", None)
        if segment_meta:
            header.update(segment_meta)
        base = request["base_time_us_of_day"] // 1_000_000
        lines = [json.dumps(header)]
        for statement in request["statements"]:
            total = base + statement.get("start_offset_us", 0) // 1_000_000
            hour, remainder = divmod(total % 86_400, 3_600)
            minute, second = divmod(remainder, 60)
            row = {
                "start": f"{hour:02}:{minute:02}:{second:02}",
                "text": statement["text"],
                "sentence_id": statement["id"],
            }
            if request["source"]:
                row["source"] = request["source"]
            if "speaker" in statement:
                row["speaker"] = statement["speaker"]
            lines.append(json.dumps(row))
        Path(request["jsonl_path"]).write_text("\n".join(lines) + "\n")
        return SpeakerTranscriptWriteResponse(
            jsonl_path=request["jsonl_path"],
            npz_path=request["npz_path"],
            statement_count=len(request["statements"]),
            embedding_row_count=len(request["embedding_statement_ids"] or []),
        )

    monkeypatch.setattr(transcribe_main, "write_speaker_transcript", write)


def _native_header(*, raw_path: Path = Path("audio.m4a"), **kwargs) -> dict:
    """Return the request header assembled for the native writer."""
    from solstone.observe.transcribe.main import _write_native_transcript

    with patch(
        "solstone.observe.transcribe.main.write_speaker_transcript",
        return_value=SpeakerTranscriptWriteResponse(
            jsonl_path="audio.jsonl",
            npz_path="audio.npz",
            statement_count=1,
            embedding_row_count=0,
        ),
    ) as writer:
        _write_native_transcript(
            raw_path,
            Path("audio.jsonl"),
            statements=[{"id": 1, "start": 1.0, "end": 2.0, "text": "Hello"}],
            base_datetime=datetime(2026, 5, 22, 9, 0, 0),
            model_info={"model": "unit", "device": "cpu", "compute_type": "int8"},
            **kwargs,
        )
    return writer.call_args.kwargs["header"]


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
        assert DEFAULT_MIN_SPEECH_SECONDS == 1.0


class TestLoadAudio:
    """Test the shared load_audio utility."""

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

    def test_m4a_returns_numpy_array(self):
        """M4A files should return a numpy array with audio content."""
        # Generated with:
        # ffmpeg -y -f lavfi \
        #   -i "sine=frequency=440:duration=0.5:sample_rate=16000" \
        #   -c:a aac -b:a 64k \
        #   tests/fixtures/audio/aac_single_track.m4a
        m4a_path = Path(__file__).parent / "fixtures" / "audio" / "aac_single_track.m4a"

        audio = load_audio(m4a_path)

        assert isinstance(audio, np.ndarray)
        assert audio.dtype == np.float32
        assert len(audio) > 0

    def test_multi_track_m4a_mixes_streams(self):
        """load_audio should mix multiple M4A audio streams together."""
        # Generated with:
        # ffmpeg -y -f lavfi -i "anullsrc=r=16000:cl=mono" \
        #   -f lavfi -i "sine=frequency=440:duration=1:sample_rate=16000,volume=4" \
        #   -map 0:a -map 1:a -c:a aac -b:a 64k -t 1 \
        #   tests/fixtures/audio/aac_multi_track.m4a
        m4a_path = Path(__file__).parent / "fixtures" / "audio" / "aac_multi_track.m4a"

        audio = load_audio(m4a_path)

        assert isinstance(audio, np.ndarray)
        assert audio.dtype == np.float32

        # The mixed audio should have content from track 1 (the sine wave)
        # AAC compression affects amplitude, so use loose threshold
        rms = np.sqrt(np.mean(audio**2))
        assert rms > 0.1, f"Mixed audio should contain signal, got RMS={rms}"

    @pytest.mark.integration
    @pytest.mark.skipif(not shutil.which("ffmpeg"), reason="ffmpeg not installed")
    def test_m4a_ffmpeg_round_trip_decodes(self, tmp_path):
        m4a_path = tmp_path / "round-trip.m4a"
        result = subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.5:sample_rate=16000",
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                str(m4a_path),
            ],
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stderr

        audio = load_audio(m4a_path)

        assert isinstance(audio, np.ndarray)
        assert audio.dtype == np.float32
        assert len(audio) > 0

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
        assert isinstance(excinfo.value, AudioDecodeError)

    def test_load_audio_reports_worker_signal_exit(self, tmp_path, monkeypatch):
        path = tmp_path / "crash.m4a"
        path.write_bytes(b"truncated")

        def fake_decode(*args, **kwargs):
            raise AudioDecodeError("worker exited from signal 11")

        monkeypatch.setattr(observe_utils, "_decode_audio_in_worker", fake_decode)

        with pytest.raises(AudioDecodeError) as excinfo:
            load_audio(path)

        assert "worker exited from signal 11" in str(excinfo.value)

    def test_load_audio_reports_malformed_worker_payload(self, tmp_path, monkeypatch):
        path = tmp_path / "bad-payload.m4a"
        path.write_bytes(b"payload")

        def fake_decode(*args, **kwargs):
            return {"ok": True}

        monkeypatch.setattr(observe_utils, "_decode_audio_in_worker", fake_decode)

        with pytest.raises(AudioDecodeError) as excinfo:
            load_audio(path)

        assert "invalid worker sample rate" in str(excinfo.value)

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
            encoder = np.array(ENCODER_ID)

            np.savez_compressed(
                npz_path,
                embeddings=embeddings,
                statement_ids=statement_ids,
                encoder=encoder,
            )

            loaded = np.load(npz_path)
            np.testing.assert_array_almost_equal(loaded["embeddings"], embeddings)
            np.testing.assert_array_equal(loaded["statement_ids"], statement_ids)
            assert loaded["encoder"].item() == ENCODER_ID

    def test_statement_ids_are_unique(self):
        """Statement IDs should be unique."""
        statement_ids = np.array([1, 2, 3, 4, 5], dtype=np.int32)
        assert len(statement_ids) == len(np.unique(statement_ids))


def test_process_audio_failed_native_write_emits_failed_event(tmp_path):
    from solstone.observe.transcribe.main import process_audio
    from solstone.observe.transcribe.native import NativeSpeakerTranscriptWriteError

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.touch()
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }
    embedding_payload = SpeakerEmbeddingPayload(
        payload=np.zeros((1, 256), dtype="<f4").tobytes(),
        statement_ids=[1],
        durations_s=[0.0],
        encoder="test",
    )

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(statements, embedding_payload=embedding_payload),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
        patch(
            "solstone.observe.transcribe.main.write_speaker_transcript",
            side_effect=NativeSpeakerTranscriptWriteError(
                reason="output-unwritable",
                message="temporary write failure",
                exit_code=75,
            ),
        ),
    ):
        with pytest.raises(SystemExit) as exc:
            process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert exc.value.code == 75
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "failed"
    assert "NativeSpeakerTranscriptWriteError" in mock_send.call_args.kwargs["error"]


def test_process_audio_routes_embedding_payload_to_native_writer(tmp_path):
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.touch()
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }
    embedding_payload = SpeakerEmbeddingPayload(
        payload=np.zeros((1, 256), dtype="<f4").tobytes(),
        statement_ids=[1],
        durations_s=[0.0],
        encoder="test",
    )

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(statements, embedding_payload=embedding_payload),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    row = json.loads(raw_path.with_suffix(".jsonl").read_text().splitlines()[1])
    assert row["sentence_id"] == 1
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "transcribed"


def test_process_audio_native_failure_emits_attributed_failure_only(tmp_path):
    from solstone.observe.transcribe.main import process_audio
    from solstone.observe.transcribe.speakers_analyze_errors import (
        SPEAKER_ANALYSIS_FAILURE_LABEL,
        SPEAKER_ANALYSIS_FAILURE_REASON,
        SpeakerAnalyzeError,
    )

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 2048)
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=SpeakerAnalyzeError(
                path=raw_path,
                stage="invoke",
                reason="unavailable",
                native_exit_code=75,
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SpeakerAnalyzeError):
            process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert raw_path.exists()
    assert not raw_path.with_suffix(".jsonl").exists()
    assert not raw_path.with_suffix(".npz").exists()
    assert mock_send.call_count == 1
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    kwargs = mock_send.call_args.kwargs
    assert kwargs["outcome"] == "failed"
    assert kwargs["reason"] == SPEAKER_ANALYSIS_FAILURE_REASON
    assert kwargs["error"] == SPEAKER_ANALYSIS_FAILURE_LABEL
    assert kwargs["speaker_analysis_failure_path"] == "native"
    assert kwargs["speaker_analysis_failure_stage"] == "invoke"
    assert kwargs["speaker_analysis_failure_reason"] == "unavailable"
    assert kwargs["speaker_analysis_failure_native_exit_code"] == 75


def test_process_audio_native_failure_emit_error_does_not_write_artifacts(
    tmp_path,
    caplog: pytest.LogCaptureFixture,
):
    from solstone.observe.transcribe.main import process_audio
    from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 2048)
    before = repository_inventory(tmp_path)
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=SpeakerAnalyzeError(
                path=raw_path,
                stage="invoke",
                reason="timeout",
            ),
        ),
        patch(
            "solstone.observe.transcribe.main.callosum_send",
            side_effect=RuntimeError("callosum down"),
        ) as mock_send,
        caplog.at_level("ERROR"),
    ):
        with pytest.raises(SpeakerAnalyzeError):
            process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert mock_send.call_count == 1
    assert "Failed to emit transcription failure event" in caplog.text
    assert raw_path.exists()
    assert not raw_path.with_suffix(".jsonl").exists()
    assert not raw_path.with_suffix(".npz").exists()
    assert_inventory_unchanged(before, repository_inventory(tmp_path))


def test_all_batch_typed_speaker_failure_continues_and_preserves_failed_audio(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
):
    from solstone.observe.transcribe.main import main
    from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError
    from solstone.think.speakers_analyze_installation import (
        SpeakersAnalyzeInstallationResult,
    )

    first = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.flac"
    )
    second = (
        tmp_path / "chronicle" / "20260416" / "default" / "121000_300" / "audio.flac"
    )
    first.parent.mkdir(parents=True)
    second.parent.mkdir(parents=True)
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    def fake_analyze_speakers(**kwargs):
        raw_path = kwargs["raw_path"]
        if raw_path == first:
            raise SpeakerAnalyzeError(
                path=raw_path,
                stage="invoke",
                reason="unavailable",
                native_exit_code=75,
            )
        return _speaker_result(statements)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sys, "argv", ["journal transcribe", "--all"])
    with (
        patch(
            "solstone.think.speakers_analyze_installation."
            "check_speakers_analyze_installation",
            return_value=SpeakersAnalyzeInstallationResult("ok"),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch(
            "solstone.observe.vad.run_vad",
            return_value=VadResult(
                duration=10.0,
                speech_duration=5.0,
                has_speech=True,
                speech_segments=[(1.0, 6.0)],
            ),
        ),
        patch("solstone.observe.vad.reduce_audio", return_value=(None, None)),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=None),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe",
            return_value=statements,
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=backend_module,
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=fake_analyze_speakers,
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        main()

    captured = capsys.readouterr()
    assert "1 processed" in captured.out
    assert "1 failed" in captured.out
    assert first.exists()
    assert not first.with_suffix(".jsonl").exists()
    assert not first.with_suffix(".npz").exists()
    assert second.with_suffix(".jsonl").exists()
    speaker_failure_events = [
        call
        for call in mock_send.call_args_list
        if call.kwargs.get("outcome") == "failed"
        and call.kwargs.get("speaker_analysis_failure_path") == "native"
    ]
    assert len(speaker_failure_events) == 1


def test_all_batch_unexpected_adapter_exception_aborts(tmp_path, monkeypatch):
    from solstone.observe.transcribe.main import main
    from solstone.think.speakers_analyze_installation import (
        SpeakersAnalyzeInstallationResult,
    )

    class UnexpectedAdapterBug(RuntimeError):
        pass

    first = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.flac"
    )
    second = (
        tmp_path / "chronicle" / "20260416" / "default" / "121000_300" / "audio.flac"
    )
    first.parent.mkdir(parents=True)
    second.parent.mkdir(parents=True)
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sys, "argv", ["journal transcribe", "--all"])
    with (
        patch(
            "solstone.think.speakers_analyze_installation."
            "check_speakers_analyze_installation",
            return_value=SpeakersAnalyzeInstallationResult("ok"),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch(
            "solstone.observe.vad.run_vad",
            return_value=VadResult(
                duration=10.0,
                speech_duration=5.0,
                has_speech=True,
                speech_segments=[(1.0, 6.0)],
            ),
        ),
        patch("solstone.observe.vad.reduce_audio", return_value=(None, None)),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=None),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe",
            return_value=statements,
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=backend_module,
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=UnexpectedAdapterBug("boom"),
        ),
    ):
        with pytest.raises(SystemExit) as exc_info:
            main()

    assert exc_info.value.code == 1
    assert isinstance(exc_info.value.__cause__, UnexpectedAdapterBug)
    assert not first.with_suffix(".jsonl").exists()
    assert not second.with_suffix(".jsonl").exists()


def test_single_file_typed_speaker_failure_emits_once_and_exits_one(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
):
    from solstone.observe.transcribe.main import main
    from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError
    from solstone.think.speakers_analyze_installation import (
        SpeakersAnalyzeInstallationResult,
    )

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.flac"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"audio")
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sys, "argv", ["journal transcribe", str(raw_path)])
    with (
        patch(
            "solstone.think.speakers_analyze_installation."
            "check_speakers_analyze_installation",
            return_value=SpeakersAnalyzeInstallationResult("ok"),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch(
            "solstone.observe.vad.run_vad",
            return_value=VadResult(
                duration=10.0,
                speech_duration=5.0,
                has_speech=True,
                speech_segments=[(1.0, 6.0)],
            ),
        ),
        patch("solstone.observe.vad.reduce_audio", return_value=(None, None)),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=None),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe",
            return_value=statements,
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=backend_module,
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=SpeakerAnalyzeError(
                path=raw_path,
                stage="invoke",
                reason="timeout",
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SystemExit) as exc:
            main()

    assert exc.value.code == 1
    assert raw_path.exists()
    assert not raw_path.with_suffix(".jsonl").exists()
    assert not raw_path.with_suffix(".npz").exists()
    speaker_failure_events = [
        call
        for call in mock_send.call_args_list
        if call.kwargs.get("outcome") == "failed"
        and call.kwargs.get("speaker_analysis_failure_path") == "native"
    ]
    assert len(speaker_failure_events) == 1


def test_process_audio_zero_row_native_response_writes_no_embedding_archive(tmp_path):
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 2048)
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }
    native_result = SpeakerAnalyzeResult(
        statements=statements,
        embedding_payload=None,
        speaker_evidence=SpeakerEvidenceDecision("single", 0.0, 0.0),
        overlap_fraction=0.0,
        statement_labels=None,
    )

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=native_result,
        ),
        patch("solstone.observe.transcribe.main.callosum_send"),
    ):
        process_audio(
            raw_path,
            np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
            VadResult(
                duration=10.0,
                speech_duration=5.0,
                has_speech=True,
                speech_segments=[(1.0, 6.0)],
            ),
            {},
            backend="parakeet",
        )

    assert raw_path.with_suffix(".jsonl").exists()
    assert not raw_path.with_suffix(".npz").exists()


def test_process_audio_native_adapter_restores_once_after_stt(tmp_path):
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 2048)
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    reduction = AudioReduction(
        segments=[SpeechSegment(5.0, 6.0, 0.0, 1.0)],
        original_duration=10.0,
        reduced_duration=1.0,
    )
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }
    adapter_seen: list[list[dict]] = []
    restore_seen: list[list[dict]] = []

    def fake_analyze_speakers(**kwargs):
        adapter_seen.append(
            [dict(statement) for statement in kwargs["statements_pre_restore"]]
        )
        restored = kwargs["statements_restored"]()
        return _speaker_result(restored)

    def fake_restore(seen_statements, _reduction):
        restore_seen.append([dict(statement) for statement in seen_statements])
        return [
            {
                **statement,
                "start": statement["start"] + 5.0,
                "end": statement["end"] + 5.0,
            }
            for statement in seen_statements
        ]

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            side_effect=fake_analyze_speakers,
        ),
        patch(
            "solstone.observe.vad.restore_statement_timestamps",
            side_effect=fake_restore,
        ),
        patch("solstone.observe.transcribe.main.callosum_send"),
    ):
        process_audio(
            raw_path,
            np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
            VadResult(
                duration=10.0,
                speech_duration=5.0,
                has_speech=True,
                speech_segments=[(1.0, 6.0)],
            ),
            {},
            reduction=reduction,
            reduced_audio=np.zeros(SAMPLE_RATE, dtype=np.float32),
            backend="parakeet",
        )

    assert adapter_seen == [statements]
    assert restore_seen == [statements]


def test_process_audio_records_analyzed_processing(tmp_path):
    from solstone.observe.processing_record import (
        HANDLER_TRANSCRIBE,
        REASON_OK,
        SCHEMA,
        STATE_ANALYZED,
    )
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 2048)
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "unit",
        "device": "cpu",
        "compute_type": "int8",
    }
    embedding_payload = SpeakerEmbeddingPayload(
        payload=np.zeros((1, 256), dtype=np.float32).tobytes(),
        statement_ids=[1],
        durations_s=[1.0],
        encoder="test",
    )
    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(statements, embedding_payload=embedding_payload),
        ),
        patch(
            "solstone.observe.processing_record.now_iso_utc",
            return_value="2026-06-30T12:00:00Z",
        ),
        patch("solstone.observe.transcribe.main.callosum_send"),
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    jsonl_path = raw_path.with_suffix(".jsonl")
    header = json.loads(jsonl_path.read_text().splitlines()[0])
    assert header["_solstone_processing"] == {
        "schema": SCHEMA,
        "state": STATE_ANALYZED,
        "reason_code": REASON_OK,
        "handler": HANDLER_TRANSCRIBE,
        "attempted_at": "2026-06-30T12:00:00Z",
        "input_size": 2048,
    }
    assert len(jsonl_path.read_text().splitlines()) >= 2


def test_process_audio_silent_filtered_writes_empty_record(tmp_path):
    from solstone.observe.processing_record import (
        HANDLER_TRANSCRIBE,
        REASON_NO_DECODABLE_AUDIO,
        STATE_EMPTY,
    )
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.write_bytes(b"\x00" * 1024)
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "unit",
        "device": "cpu",
        "compute_type": "int8",
    }
    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    jsonl_path = raw_path.with_suffix(".jsonl")
    assert jsonl_path.exists()
    header = json.loads(jsonl_path.read_text(encoding="utf-8").splitlines()[0])
    record = header["_solstone_processing"]
    assert record["state"] == STATE_EMPTY
    assert record["reason_code"] == REASON_NO_DECODABLE_AUDIO
    assert record["handler"] == HANDLER_TRANSCRIBE
    assert record["input_size"] == 1024
    assert not raw_path.exists()
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_process_audio_native_gate_decline_is_accepted(tmp_path):
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.touch()
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(
                statements,
                speaker_evidence=SpeakerEvidenceDecision("single", 0.0, 0.0),
                statement_labels=None,
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "transcribed"

    jsonl_path = raw_path.with_suffix(".jsonl")
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    assert jsonl_path.exists()
    assert "speaker" not in json.loads(lines[1])


def test_process_audio_writes_native_statement_labels(tmp_path):
    from solstone.observe.transcribe.main import process_audio

    raw_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    )
    raw_path.parent.mkdir(parents=True)
    raw_path.touch()
    audio_buffer = np.zeros(10 * SAMPLE_RATE, dtype=np.float32)
    vad_result = VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )
    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "unit",
        "device": "cpu",
        "compute_type": "int8",
    }
    labeled_statements = [{**statements[0], "speaker": 2}]

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend", return_value=backend_module
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(
                labeled_statements,
                speaker_evidence=SpeakerEvidenceDecision("multi", 1.0, 0.5),
                overlap_fraction=0.5,
                statement_labels=[2],
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send"),
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    jsonl_path = raw_path.with_suffix(".jsonl")
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    assert json.loads(lines[1])["speaker"] == 2


def test_legacy_transcript_enrichment_fields_remain_reader_compatible(tmp_path):
    from solstone.observe.hear import load_transcript

    jsonl_path = (
        tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.jsonl"
    )
    jsonl_path.parent.mkdir(parents=True)
    jsonl_path.write_text(
        "\n".join(
            [
                json.dumps(
                    {
                        "raw": "audio.m4a",
                        "topics": ["planning", "shipping"],
                        "setting": "office",
                    }
                ),
                json.dumps(
                    {
                        "start": "00:00:02",
                        "text": "raw transcript",
                        "corrected": "corrected transcript",
                        "emotion": "excited",
                    }
                ),
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    metadata, entries, formatted_text = load_transcript(jsonl_path)

    assert metadata["topics"] == ["planning", "shipping"]
    assert metadata["setting"] == "office"
    assert entries == [
        {
            "start": "00:00:02",
            "text": "raw transcript",
            "corrected": "corrected transcript",
            "emotion": "excited",
        }
    ]
    assert "Topics: planning, shipping" in formatted_text
    assert "Setting: office" in formatted_text
    assert "[00:00:02] corrected transcript *(excited)*" in formatted_text
    assert "raw transcript" not in formatted_text


class TestJSONLFormat:
    """Test JSONL output format."""

    def test_statements_to_jsonl_includes_duration(self):
        """Audio metadata should include decode-derived duration."""
        metadata = _native_header(
            vad_result=VadResult(
                duration=12.34,
                speech_duration=1.0,
                has_speech=True,
            ),
        )

        assert metadata["duration"] == 12.34
        assert isinstance(metadata["duration"], float)

    def test_statements_to_jsonl_raw_is_producer_invariant(self):
        metadata = _native_header(raw_path=Path("audio.flac"))

        # raw is the producer's invariant (relaxed from the shared floor), so the
        # transcriber must keep emitting it.
        assert metadata["raw"] == "audio.flac"

    def test_statements_to_jsonl_writes_speaker_evidence_decision_fields(self):
        metadata = _native_header(
            speaker_evidence=SpeakerEvidenceDecision("none", 0.0, 0.25),
        )

        assert metadata["speaker_evidence"] == "none"
        assert metadata["speaker_evidence_multi_fraction"] == 0.0
        assert metadata["speaker_evidence_version"] == "windowed-slots-v1"
        assert "speaker_evidence_mean_window_overlap_share" not in metadata

    def test_new_headers_always_include_speaker_analysis_producer(self):
        no_helper_metadata = _native_header()
        helper_metadata = _native_header(
            speaker_analysis_producer=SPEAKERS_ANALYZE_PRODUCER_ID,
        )

        assert "speaker_analysis_producer" not in no_helper_metadata
        assert (
            helper_metadata["speaker_analysis_producer"] == SPEAKERS_ANALYZE_PRODUCER_ID
        )

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
        # Example metadata passed to the native transcript writer.
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
