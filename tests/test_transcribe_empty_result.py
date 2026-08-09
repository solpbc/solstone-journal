# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for empty-result handling in process_audio."""

import argparse
import datetime
import importlib
import json
import logging
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np
import pytest

from solstone.apps.speakers.evidence import SpeakerEvidenceDecision
from solstone.observe.processing_record import (
    HANDLER_TRANSCRIBE,
    REASON_NO_DECODABLE_AUDIO,
    SCHEMA,
    STATE_EMPTY,
)
from solstone.observe.transcribe.native import (
    NativeSpeakerTranscriptWriteError,
    SpeakerTranscriptWriteResponse,
)
from solstone.observe.transcribe.speakers_analyze_adapter import SpeakerAnalyzeResult
from solstone.observe.utils import SAMPLE_RATE
from solstone.observe.vad import VadResult
from solstone.think.data_state import (
    DataState,
    derive_modality_state,
    read_processing_record,
)

SOUND_TAGS = {
    "engine": "ced.cpp v0.1.0",
    "model": "ced-tiny-q8_0",
    "threshold": 0.1,
    "window_s": 10,
    "agg": "max",
    "windows": 1,
    "tags": {"Music": 0.201, "Silence": 0.5},
}


def _speaker_result(statements: list[dict]) -> SpeakerAnalyzeResult:
    return SpeakerAnalyzeResult(
        statements=[dict(statement) for statement in statements],
        embedding_payload=None,
        speaker_evidence=SpeakerEvidenceDecision("single", 0.0, 0.0),
        overlap_fraction=0.0,
        statement_labels=None,
    )


@pytest.fixture(autouse=True)
def _native_transcript_writer(monkeypatch: pytest.MonkeyPatch):
    """Keep empty-result handler tests independent of a built native helper."""
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    def write(**request):
        header = dict(request["header"])
        segment_meta = header.pop("segment_meta", None)
        if segment_meta:
            header.update(segment_meta)
        Path(request["jsonl_path"]).write_text(json.dumps(header) + "\n")
        return SpeakerTranscriptWriteResponse(
            jsonl_path=request["jsonl_path"],
            npz_path=request["npz_path"],
            statement_count=len(request["statements"]),
            embedding_row_count=len(request["embedding_statement_ids"] or []),
        )

    monkeypatch.setattr(transcribe_main, "write_speaker_transcript", write)


SILENCE_SOUND_TAGS = {
    "engine": "ced.cpp v0.1.0",
    "model": "ced-tiny-q8_0",
    "threshold": 0.1,
    "window_s": 10,
    "agg": "max",
    "windows": 1,
    "tags": {"Silence": 0.7, "White noise": 0.3},
}


@pytest.fixture
def raw_path(tmp_path):
    path = tmp_path / "chronicle" / "20260416" / "default" / "120000_300" / "audio.m4a"
    path.parent.mkdir(parents=True)
    path.touch()
    return path


@pytest.fixture
def audio_buffer():
    return np.zeros(10 * SAMPLE_RATE, dtype=np.float32)


@pytest.fixture
def vad_result():
    return VadResult(
        duration=10.0,
        speech_duration=5.0,
        has_speech=True,
        speech_segments=[(1.0, 6.0)],
    )


@pytest.fixture
def no_speech_vad_result():
    return VadResult(
        duration=10.0,
        speech_duration=0.0,
        has_speech=False,
        speech_segments=[],
    )


def _backend_module() -> MagicMock:
    backend_module = MagicMock()
    backend_module.get_model_info.return_value = {
        "model": "medium.en",
        "device": "cpu",
        "compute_type": "int8",
    }
    return backend_module


def _read_header(jsonl_path):
    return json.loads(jsonl_path.read_text(encoding="utf-8").splitlines()[0])


def _read_jsonl_lines(jsonl_path):
    return jsonl_path.read_text(encoding="utf-8").splitlines()


def _assert_empty_record(header, *, input_size=None):
    record = header["_solstone_processing"]
    assert record["schema"] == SCHEMA
    assert record["state"] == STATE_EMPTY
    assert record["reason_code"] == REASON_NO_DECODABLE_AUDIO
    assert record["handler"] == HANDLER_TRANSCRIBE
    if input_size is not None:
        assert record["input_size"] == input_size
    return record


def test_process_audio_speech_writes_sound_tags_and_keeps_audio(
    raw_path,
    audio_buffer,
    vad_result,
):
    from solstone.observe.transcribe.main import process_audio

    statements = [{"id": 1, "start": 0.0, "end": 1.0, "text": "hi"}]

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
            return_value=_backend_module(),
        ),
        patch(
            "solstone.observe.transcribe.speakers_analyze_adapter.analyze_speakers",
            return_value=_speaker_result(statements),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(
            raw_path,
            audio_buffer,
            vad_result,
            {},
            backend="parakeet",
            sound_tags=SOUND_TAGS,
        )

    assert raw_path.exists()
    header = _read_header(raw_path.with_suffix(".jsonl"))
    assert header["sound_tags"] == SOUND_TAGS
    assert mock_send.call_args.kwargs["outcome"] == "transcribed"


def test_zero_statement_header_omits_speaker_analysis_producer(
    raw_path, audio_buffer, vad_result
):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=_backend_module(),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert not raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    assert jsonl_path.exists()
    header = _read_header(jsonl_path)
    _assert_empty_record(header)
    assert "speaker_analysis_producer" not in header
    assert "sound_tags" not in header
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_empty_statements_preserve_path(raw_path, audio_buffer, vad_result):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": True}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=_backend_module(),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(
            raw_path,
            audio_buffer,
            vad_result,
            {},
            backend="parakeet",
            sound_tags=SOUND_TAGS,
        )

    assert raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0])
    record = header["_solstone_processing"]
    assert len(lines) == 1
    assert header["sound_tags"] == SOUND_TAGS
    assert record["state"] == STATE_EMPTY
    assert record["reason_code"] == REASON_NO_DECODABLE_AUDIO
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "preserved"


def test_empty_statements_with_tags_writes_empty_jsonl_then_deletes_audio(
    raw_path,
    audio_buffer,
    vad_result,
):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=_backend_module(),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(
            raw_path,
            audio_buffer,
            vad_result,
            {},
            backend="parakeet",
            sound_tags=SOUND_TAGS,
        )

    jsonl_path = raw_path.with_suffix(".jsonl")
    assert not raw_path.exists()
    assert jsonl_path.exists()
    assert _read_header(jsonl_path)["sound_tags"] == SOUND_TAGS
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_empty_statements_non_salient_writes_empty_record_then_deletes_audio(
    raw_path,
    audio_buffer,
    vad_result,
):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=_backend_module(),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        process_audio(
            raw_path,
            audio_buffer,
            vad_result,
            {},
            backend="parakeet",
            sound_tags=SILENCE_SOUND_TAGS,
        )

    assert not raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    assert jsonl_path.exists()
    header = _read_header(jsonl_path)
    _assert_empty_record(header)
    assert header["sound_tags"] == SILENCE_SOUND_TAGS
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_empty_statements_write_failure_preserves_audio(
    raw_path,
    audio_buffer,
    vad_result,
):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.get_config",
            return_value={"transcribe": {"preserve_all": False}},
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
        patch(
            "solstone.observe.transcribe.main.get_backend",
            return_value=_backend_module(),
        ),
        patch(
            "solstone.observe.transcribe.main.write_speaker_transcript",
            side_effect=NativeSpeakerTranscriptWriteError(
                reason="output-unwritable",
                message="disk full",
                exit_code=75,
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SystemExit):
            process_audio(
                raw_path,
                audio_buffer,
                vad_result,
                {},
                backend="parakeet",
                sound_tags=SOUND_TAGS,
            )

    assert raw_path.exists()
    assert not raw_path.with_suffix(".jsonl").exists()
    assert mock_send.call_args.kwargs["outcome"] == "failed"


def test_vad_no_speech_preserve_path_writes_empty_record(
    raw_path,
    no_speech_vad_result,
):
    from solstone.observe.transcribe.main import _process_one

    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)

    with (
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=SOUND_TAGS),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        _process_one(
            raw_path,
            args,
            {"preserve_all": True},
            "parakeet",
        )

    assert raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    lines = jsonl_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0])
    record = header["_solstone_processing"]
    assert len(lines) == 1
    assert header["sound_tags"] == SOUND_TAGS
    assert record["state"] == STATE_EMPTY
    assert record["reason_code"] == REASON_NO_DECODABLE_AUDIO
    assert header["backend"] == "unknown"
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "preserved"


def test_vad_no_speech_with_tags_writes_empty_jsonl_then_deletes_audio(
    raw_path,
    no_speech_vad_result,
):
    from solstone.observe.transcribe.main import _process_one

    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)

    with (
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=SOUND_TAGS),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        _process_one(
            raw_path,
            args,
            {"preserve_all": False},
            "parakeet",
        )

    jsonl_path = raw_path.with_suffix(".jsonl")
    assert not raw_path.exists()
    assert jsonl_path.exists()
    assert _read_header(jsonl_path)["sound_tags"] == SOUND_TAGS
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_vad_no_speech_non_salient_writes_empty_record_then_deletes_audio(
    raw_path,
    no_speech_vad_result,
):
    from solstone.observe.transcribe.main import _process_one

    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)

    with (
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
        patch(
            "solstone.observe.transcribe.main.tag_audio",
            return_value=SILENCE_SOUND_TAGS,
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        _process_one(
            raw_path,
            args,
            {"preserve_all": False},
            "parakeet",
        )

    assert not raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    assert jsonl_path.exists()
    header = _read_header(jsonl_path)
    _assert_empty_record(header)
    assert header["sound_tags"] == SILENCE_SOUND_TAGS
    assert mock_send.call_args.kwargs["outcome"] == "filtered"


def test_vad_no_speech_write_failure_preserves_audio(
    raw_path,
    no_speech_vad_result,
):
    from solstone.observe.transcribe.main import _process_one

    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)

    with (
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=SOUND_TAGS),
        patch(
            "solstone.observe.transcribe.main.write_speaker_transcript",
            side_effect=NativeSpeakerTranscriptWriteError(
                reason="output-unwritable",
                message="disk full",
                exit_code=75,
            ),
        ),
        patch("solstone.observe.transcribe.main.callosum_send"),
    ):
        with pytest.raises(SystemExit) as exc:
            _process_one(
                raw_path,
                args,
                {"preserve_all": False},
                "parakeet",
            )

    assert exc.value.code == 75
    assert raw_path.exists()
    assert not raw_path.with_suffix(".jsonl").exists()


def test_vad_no_speech_tagger_raise_writes_empty_record_without_tags(
    raw_path,
    no_speech_vad_result,
    caplog: pytest.LogCaptureFixture,
):
    from solstone.observe.transcribe.main import _process_one

    args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)
    caplog.set_level(logging.WARNING)

    with (
        patch(
            "solstone.observe.transcribe.main.load_audio",
            return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
        ),
        patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
        patch(
            "solstone.observe.transcribe.main.tag_audio",
            side_effect=RuntimeError("tagger bug"),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        _process_one(
            raw_path,
            args,
            {"preserve_all": False},
            "parakeet",
        )

    assert not raw_path.exists()
    jsonl_path = raw_path.with_suffix(".jsonl")
    assert jsonl_path.exists()
    header = _read_header(jsonl_path)
    _assert_empty_record(header)
    assert "sound_tags" not in header
    assert mock_send.call_args.kwargs["outcome"] == "filtered"
    warnings = [
        record
        for record in caplog.records
        if record.levelno == logging.WARNING
        and "sound tagging failed" in record.message
    ]
    assert len(warnings) == 1


@pytest.mark.parametrize(
    ("branch", "sound_tags"),
    [
        pytest.param("empty_statements", SOUND_TAGS, id="empty-statements-tags"),
        pytest.param("empty_statements", None, id="empty-statements-no-tags"),
        pytest.param("vad_no_speech", SOUND_TAGS, id="vad-no-speech-tags"),
        pytest.param("vad_no_speech", None, id="vad-no-speech-no-tags"),
    ],
)
def test_filtered_raw_deletion_leaves_terminal_processing_record(
    raw_path,
    audio_buffer,
    vad_result,
    no_speech_vad_result,
    branch,
    sound_tags,
):
    raw_path.write_bytes(b"raw audio bytes")
    input_size = raw_path.stat().st_size

    if branch == "empty_statements":
        from solstone.observe.transcribe.main import process_audio

        with (
            patch(
                "solstone.observe.transcribe.main.get_config",
                return_value={"transcribe": {"preserve_all": False}},
            ),
            patch(
                "solstone.observe.transcribe.main.get_journal",
                return_value=str(raw_path.parents[4]),
            ),
            patch("solstone.observe.transcribe.main.stt_transcribe", return_value=[]),
            patch(
                "solstone.observe.transcribe.main.get_backend",
                return_value=_backend_module(),
            ),
            patch("solstone.observe.transcribe.main.callosum_send"),
        ):
            process_audio(
                raw_path,
                audio_buffer,
                vad_result,
                {},
                backend="parakeet",
                sound_tags=sound_tags,
            )
    else:
        from solstone.observe.transcribe.main import _process_one

        args = argparse.Namespace(backend=None, cpu=False, model=None, redo=False)
        with (
            patch(
                "solstone.observe.transcribe.main.load_audio",
                return_value=np.zeros(10 * SAMPLE_RATE, dtype=np.float32),
            ),
            patch("solstone.observe.vad.run_vad", return_value=no_speech_vad_result),
            patch(
                "solstone.observe.transcribe.main.tag_audio", return_value=sound_tags
            ),
            patch("solstone.observe.transcribe.main.callosum_send"),
        ):
            _process_one(
                raw_path,
                args,
                {"preserve_all": False},
                "parakeet",
            )

    jsonl_path = raw_path.with_suffix(".jsonl")
    assert not raw_path.exists()
    assert jsonl_path.exists()
    lines = _read_jsonl_lines(jsonl_path)
    assert len(lines) == 1

    header = json.loads(lines[0])
    record = header["_solstone_processing"]
    attempted_at = record["attempted_at"]
    assert isinstance(attempted_at, str)
    datetime.datetime.strptime(attempted_at, "%Y-%m-%dT%H:%M:%SZ")
    assert record == {
        "schema": SCHEMA,
        "state": STATE_EMPTY,
        "reason_code": REASON_NO_DECODABLE_AUDIO,
        "handler": HANDLER_TRANSCRIBE,
        "attempted_at": attempted_at,
        "input_size": input_size,
    }
    if sound_tags is None:
        assert "sound_tags" not in header
    else:
        assert header["sound_tags"] == sound_tags

    assert read_processing_record([header]) == record
    assert (
        derive_modality_state(
            raw_path.parent,
            "audio",
            has_chunks=False,
            has_jsonl=True,
            has_raw=False,
            record=record,
        )
        == DataState.EMPTY.value
    )


def test_backend_raise_propagates(raw_path, audio_buffer, vad_result):
    from solstone.observe.transcribe.main import process_audio

    with (
        patch(
            "solstone.observe.transcribe.main.stt_transcribe",
            side_effect=RuntimeError("transcription backend 502"),
        ),
        patch(
            "solstone.observe.transcribe.main.get_journal",
            return_value=str(raw_path.parents[4]),
        ),
        patch("solstone.observe.transcribe.main.callosum_send") as mock_send,
    ):
        with pytest.raises(SystemExit) as exc_info:
            process_audio(raw_path, audio_buffer, vad_result, {}, backend="parakeet")

    assert exc_info.value.code == 1
    assert raw_path.exists()
    assert mock_send.call_args.args[:2] == ("observe", "transcribed")
    assert mock_send.call_args.kwargs["outcome"] == "failed"
    assert mock_send.call_args.kwargs["backend"] == "parakeet"
    assert (
        mock_send.call_args.kwargs["input"] == "20260416/default/120000_300/audio.m4a"
    )
    # The event carries the exception TYPE, never its message: messages can embed
    # model output (see _failure_label). The message stays in the handler log.
    assert mock_send.call_args.kwargs["error"] == "RuntimeError"
    assert mock_send.call_args.kwargs["reason"] == "RuntimeError"
    assert "transcription backend 502" not in json.dumps(
        mock_send.call_args.kwargs, default=str
    )
