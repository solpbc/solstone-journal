# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stage telemetry on the observe.transcribed event.

See solstone/observe/transcribe/failure-and-telemetry.md for the field contract.
"""

from __future__ import annotations

import argparse
import importlib
import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
import pytest

from solstone.apps.speakers.evidence import SpeakerEvidenceDecision
from solstone.observe.transcribe.native import SpeakerTranscriptWriteResponse
from solstone.observe.transcribe.speakers_analyze_adapter import SpeakerAnalyzeResult
from solstone.observe.utils import SAMPLE_RATE
from solstone.observe.vad import VadResult
from solstone.think.providers.parakeet_server import ParakeetServerNotReady
from tests.helpers.module_mocks import module_mock


def _speaker_result(statements: list[dict]) -> SpeakerAnalyzeResult:
    return SpeakerAnalyzeResult(
        statements=[dict(statement) for statement in statements],
        embedding_payload=None,
        speaker_evidence=SpeakerEvidenceDecision("none", 0.0, 0.0),
        overlap_fraction=0.0,
        statement_labels=None,
    )


@pytest.fixture(autouse=True)
def _native_transcript_writer(monkeypatch: pytest.MonkeyPatch):
    """Keep telemetry assertions focused on the Python event boundary."""
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    def write(**request):
        header = dict(request["header"])
        segment_meta = header.pop("segment_meta", None)
        if segment_meta:
            header.update(segment_meta)
        lines = [json.dumps(header)]
        for statement in request["statements"]:
            lines.append(
                json.dumps(
                    {
                        "start": "00:00:00",
                        "text": statement["text"],
                        "sentence_id": statement["id"],
                    }
                )
            )
        Path(request["jsonl_path"]).write_text("\n".join(lines) + "\n")
        return SpeakerTranscriptWriteResponse(
            jsonl_path=request["jsonl_path"],
            npz_path=request["npz_path"],
            statement_count=len(request["statements"]),
            embedding_row_count=len(request["embedding_statement_ids"] or []),
        )

    monkeypatch.setattr(transcribe_main, "write_speaker_transcript", write)


@pytest.fixture
def raw_path(tmp_path: Path) -> Path:
    path = tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    path.parent.mkdir(parents=True)
    path.write_bytes(b"audio")
    return path


@pytest.fixture
def audio_buffer() -> np.ndarray:
    return np.zeros(10 * SAMPLE_RATE, dtype=np.float32)


@pytest.fixture
def vad_result() -> VadResult:
    return VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )


def _backend_module() -> MagicMock:
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "parakeet-v3-q8_0.gguf",
        "device": "gpu",
        "compute_type": "q8_0",
    }
    return backend_module


def _run_success(
    raw_path: Path,
    audio_buffer,
    vad_result,
    backend_module: MagicMock | None = None,
) -> dict:
    """Run a successful process_audio and return the emitted event kwargs."""
    from solstone.observe.transcribe.main import process_audio

    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hello"}]

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe", return_value=statements
        ),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=backend_module or _backend_module(),
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(statements),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet-cpp")

    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    return mock_send.call_args.kwargs


def _run_parakeet_cpp_process_one_event(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer,
    vad_result,
    *,
    stt_error: Exception,
    expected_exit: int,
    placement: str | None = None,
    configured_device: str | None = "auto",
) -> dict:
    from solstone.observe.transcribe import _parakeet_cpp as parakeet_cpp
    from solstone.observe.transcribe.main import _process_one

    journal_root = raw_path.parents[4]
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_root))
    if placement is not None:
        parakeet_cpp.parakeet_server.write_parakeet_placement(placement)
    parakeet_cpp_config = {}
    if configured_device is not None:
        parakeet_cpp_config["device"] = configured_device
    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)

    with (
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(journal_root),
        ),
        patch("solstone.observe.transcribe.main.load_audio", return_value=audio_buffer),
        patch("solstone.observe.vad.run_vad", return_value=vad_result),
        patch("solstone.observe.vad.reduce_audio", return_value=(None, None)),
        patch("solstone.observe.transcribe.main.tag_audio", return_value={"tags": {}}),
        patch(
            "solstone.observe.transcribe.main.stt_transcribe",
            side_effect=stt_error,
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SystemExit) as exc_info:
            _process_one(
                raw_path,
                args,
                {"backend": "parakeet-cpp", "parakeet-cpp": parakeet_cpp_config},
                "parakeet-cpp",
            )

    assert exc_info.value.code == expected_exit
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    return mock_send.call_args.kwargs


def test_success_event_carries_stage_timings_and_envelope(
    raw_path: Path, audio_buffer: np.ndarray, vad_result: VadResult
) -> None:
    kwargs = _run_success(raw_path, audio_buffer, vad_result)

    assert kwargs["outcome"] == "transcribed"
    timings = kwargs["timings"]
    # Stages that ran inside process_audio. decode/vad/reduce are measured in
    # _process_one, which this test calls past; the speaker decision is native.
    assert {"asr_ms", "speakers_analyze_ms", "write_ms"} <= set(timings)
    assert "embed_ms" not in timings
    assert "overlap_ms" not in timings
    assert "diarize_ms" not in timings
    assert all(isinstance(v, int) and v >= 0 for v in timings.values())

    assert kwargs["backend"] == "parakeet-cpp"
    assert kwargs["device"] == "gpu"
    assert kwargs["model"] == "parakeet-v3-q8_0.gguf"
    header = json.loads(raw_path.with_suffix(".jsonl").read_text().splitlines()[0])
    assert header["device"] == "gpu"
    assert kwargs["audio_seconds"] == 10.0
    assert isinstance(kwargs["peak_rss_mib"], int)
    assert kwargs["peak_rss_mib"] > 0


@pytest.mark.parametrize("placement", ["gpu", "cpu"])
def test_success_event_and_header_use_parakeet_placement_record(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
    placement: str,
) -> None:
    from solstone.observe.transcribe import _parakeet_cpp as parakeet_cpp

    monkeypatch.setattr(parakeet_cpp.sys, "platform", "linux")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(raw_path.parents[4]))
    parakeet_cpp.parakeet_server.write_parakeet_placement(placement)
    backend_module = MagicMock()
    backend_module.get_model_info.side_effect = parakeet_cpp.get_model_info

    kwargs = _run_success(raw_path, audio_buffer, vad_result, backend_module)

    assert kwargs["device"] == placement
    assert kwargs["device"] != "auto"
    header = json.loads(raw_path.with_suffix(".jsonl").read_text().splitlines()[0])
    assert header["device"] == placement
    assert header["device"] != "auto"


@pytest.mark.parametrize("placement", ["cpu", "gpu"])
def test_deferred_event_uses_parakeet_placement_record(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
    placement: str,
) -> None:
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    kwargs = _run_parakeet_cpp_process_one_event(
        monkeypatch,
        raw_path,
        audio_buffer,
        vad_result,
        stt_error=ParakeetServerNotReady("warming", retry_reason="no_port"),
        expected_exit=EXIT_PROVIDER_BLOCKED,
        placement=placement,
        configured_device="auto",
    )

    assert kwargs["outcome"] == "deferred"
    assert kwargs["backend"] == "parakeet-cpp"
    assert kwargs["device"] == placement
    assert kwargs["device"] != "auto"
    assert "model" not in kwargs


def test_deferred_event_uses_configured_device_without_parakeet_placement_record(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
) -> None:
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    kwargs = _run_parakeet_cpp_process_one_event(
        monkeypatch,
        raw_path,
        audio_buffer,
        vad_result,
        stt_error=ParakeetServerNotReady("warming", retry_reason="no_port"),
        expected_exit=EXIT_PROVIDER_BLOCKED,
        placement=None,
        configured_device="auto",
    )

    assert kwargs["outcome"] == "deferred"
    assert kwargs["device"] == "auto"


def test_deferred_event_reports_parakeet_placement_when_config_has_no_device(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
) -> None:
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    kwargs = _run_parakeet_cpp_process_one_event(
        monkeypatch,
        raw_path,
        audio_buffer,
        vad_result,
        stt_error=ParakeetServerNotReady("warming", retry_reason="no_port"),
        expected_exit=EXIT_PROVIDER_BLOCKED,
        placement="cpu",
        configured_device=None,
    )

    assert kwargs["outcome"] == "deferred"
    assert kwargs["device"] == "cpu"


@pytest.mark.parametrize("placement", ["cpu", "gpu"])
def test_failed_event_uses_parakeet_placement_record(
    monkeypatch: pytest.MonkeyPatch,
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
    placement: str,
) -> None:
    kwargs = _run_parakeet_cpp_process_one_event(
        monkeypatch,
        raw_path,
        audio_buffer,
        vad_result,
        stt_error=RuntimeError("boom"),
        expected_exit=1,
        placement=placement,
        configured_device="auto",
    )

    assert kwargs["outcome"] == "failed"
    assert kwargs["backend"] == "parakeet-cpp"
    assert kwargs["device"] == placement
    assert kwargs["device"] != "auto"


def test_failed_event_reports_exception_type(
    raw_path: Path, audio_buffer: np.ndarray, vad_result: VadResult
) -> None:
    from solstone.observe.transcribe.main import process_audio

    error = RuntimeError("provider failed")

    with (
        patch("solstone.observe.transcribe.main.stt_transcribe", side_effect=error),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SystemExit) as exc_info:
            process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert exc_info.value.code == 1
    kwargs = mock_send.call_args.kwargs
    assert kwargs["outcome"] == "failed"
    assert kwargs["reason"] == "RuntimeError"
    assert kwargs["error"] == "RuntimeError"


def test_rtfx_derived_from_asr_time(
    raw_path: Path, audio_buffer: np.ndarray, vad_result: VadResult
) -> None:
    kwargs = _run_success(raw_path, audio_buffer, vad_result)

    asr_ms = kwargs["timings"]["asr_ms"]
    if asr_ms:
        assert kwargs["rtfx"] == pytest.approx(
            kwargs["audio_seconds"] / (asr_ms / 1000), rel=0.01
        )
    else:
        # A sub-millisecond mocked ASR cannot produce an honest ratio, so none is
        # fabricated.
        assert "rtfx" not in kwargs


def test_queue_wait_is_read_from_env(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from solstone.observe.transcribe.main import _read_queue_wait_ms

    monkeypatch.setenv("SOL_QUEUE_WAIT_MS", "4200")
    assert _read_queue_wait_ms() == 4200

    monkeypatch.setenv("SOL_QUEUE_WAIT_MS", "not-a-number")
    assert _read_queue_wait_ms() is None

    monkeypatch.delenv("SOL_QUEUE_WAIT_MS")
    assert _read_queue_wait_ms() is None


def test_stage_timings_accumulate_repeated_stages(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """write_ms covers the jsonl AND the npz, so a second entry must sum, not clobber."""
    # NB: `solstone.observe.transcribe.main` as an attribute path resolves to the
    # re-exported main() *function*, not the module -- import it explicitly.
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    # Drive perf_counter so the two blocks have distinct, known durations.
    # Values are binary-exact so the int() truncation is not off by a millisecond.
    monkeypatch.setattr(
        transcribe_main,
        "time",
        module_mock(
            transcribe_main.time,
            perf_counter=MagicMock(side_effect=[0.0, 0.25, 1.0, 1.5]),
        ),
    )

    timings = transcribe_main._StageTimings()
    assert timings.as_dict() == {}

    with timings.time("write"):  # 250 ms
        pass
    assert timings.get_ms("write") == 250

    with timings.time("write"):  # 500 ms
        pass
    assert timings.get_ms("write") == 750  # summed, not clobbered to 500
    assert set(timings.as_dict()) == {"write_ms"}


def test_stage_timing_recorded_even_when_the_stage_raises() -> None:
    """A server that dies mid-ASR must still report how long ASR ran before dying."""
    from solstone.observe.transcribe.main import _StageTimings

    timings = _StageTimings()
    with pytest.raises(RuntimeError):
        with timings.time("asr"):
            raise RuntimeError("server died")

    assert timings.get_ms("asr") is not None
