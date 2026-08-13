# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Offline coverage for bad media paths that survive the describe retirement."""

from __future__ import annotations

import argparse
import importlib
import io
import json
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import numpy as np
import pytest
import soundfile as sf

from solstone.observe.processing_record import (
    HANDLER_TRANSCRIBE,
    REASON_CORRUPT_INPUT,
    REASON_NO_DECODABLE_AUDIO,
    STATE_EMPTY,
    STATE_FAILED,
)
from solstone.observe.utils import SAMPLE_RATE, AudioDecodeError
from solstone.observe.vad import VadResult
from solstone.think.cluster import cluster_segments, read_segment_data_state
from solstone.think.data_state import DataState
from solstone.think.pipeline_health import (
    classify_segment_completion,
    read_segment_progress,
)
from tests.observer_registration_helpers import register_bound_observer

DAY = "20990501"
STREAM = "default"
SEGMENT = "120000_300"
FIXED_NOW = "2026-06-30T12:00:00Z"
TEST_PL_FINGERPRINT = "sha256:" + ("d" * 64)


@pytest.fixture
def observer_env(tmp_path, monkeypatch):
    """Temp journal + Flask test client factory."""

    def _create():
        journal = tmp_path / "journal"
        journal.mkdir()
        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        (config_dir / "journal.json").write_text(
            json.dumps({"setup": {"completed_at": 1700000000000}}, indent=2)
        )
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        from solstone.convey import create_app
        from solstone.convey import root as convey_root
        from solstone.think.link.auth import AuthorizedClients
        from solstone.think.link.paths import authorized_clients_path

        app = create_app(journal=str(journal))
        authorized = AuthorizedClients(authorized_clients_path())
        authorized.add(
            TEST_PL_FINGERPRINT,
            "bad-media-observer",
            "instance-1",
            paired_at="2026-05-20T00:00:00Z",
        )
        monkeypatch.setattr(convey_root, "get_authorized_clients", lambda: authorized)

        class Env:
            def __init__(self):
                self.journal = journal
                self.client = app.test_client()

        return Env()

    return _create


@pytest.fixture
def segment_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    return journal


def _segment_dir(journal: Path) -> Path:
    path = journal / "chronicle" / DAY / STREAM / SEGMENT
    path.mkdir(parents=True, exist_ok=True)
    return path


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _read_processing_record(path: Path) -> dict[str, Any]:
    record = _read_jsonl(path)[0]["_solstone_processing"]
    assert isinstance(record, dict)
    return record


def _assert_processing_record(
    record: dict[str, Any], *, state: str, reason_code: str
) -> None:
    assert record["state"] == state
    assert record["reason_code"] == reason_code
    assert record["handler"] == HANDLER_TRANSCRIBE
    assert record["attempted_at"] == FIXED_NOW


def _write_silent_flac(path: Path, seconds: float = 0.5) -> None:
    sf.write(
        path,
        np.zeros(int(seconds * SAMPLE_RATE), np.float32),
        SAMPLE_RATE,
        format="FLAC",
    )


def _silent_flac_bytes(seconds: float = 0.5) -> bytes:
    buffer = io.BytesIO()
    sf.write(
        buffer,
        np.zeros(int(seconds * SAMPLE_RATE), np.float32),
        SAMPLE_RATE,
        format="FLAC",
    )
    return buffer.getvalue()


def _drive_transcribe(
    monkeypatch, audio_path: Path, *, preserve_all: bool
) -> tuple[dict[str, Any] | None, Mock, Path]:
    from solstone.observe import processing_record

    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    vad_module = importlib.import_module("solstone.observe.vad")
    stt_spy = Mock(return_value=[])
    monkeypatch.setattr(processing_record, "now_iso_utc", lambda: FIXED_NOW)
    monkeypatch.setattr(transcribe_main, "callosum_send", lambda *args, **kwargs: None)
    monkeypatch.setattr(transcribe_main, "stt_transcribe", stt_spy)
    monkeypatch.setattr(
        vad_module,
        "run_vad",
        lambda _audio, **_kwargs: VadResult(
            duration=0.5,
            speech_duration=0.0,
            has_speech=False,
        ),
    )
    monkeypatch.setattr(transcribe_main, "tag_audio", lambda *_args, **_kwargs: None)

    transcribe_main._process_one(
        audio_path,
        argparse.Namespace(backend=None, cpu=False, model=None, redo=False),
        {"preserve_all": preserve_all},
        "parakeet",
    )

    jsonl_path = audio_path.with_suffix(".jsonl")
    record = _read_processing_record(jsonl_path) if jsonl_path.exists() else None
    return record, stt_spy, jsonl_path


def _create_observer(env, name: str) -> str:
    response = register_bound_observer(env.client, name, TEST_PL_FINGERPRINT)
    assert response.status_code == 200
    return response.get_json()["key"]


def _pl_identity():
    from solstone.convey.secure_listener import ConveyIdentity

    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=TEST_PL_FINGERPRINT,
        device_label="bad-media-observer",
        paired_at="2026-05-20T00:00:00Z",
        session_id="session-1",
    )


def test_ac1_ingest_drops_zero_byte_keeps_valid_media(observer_env):
    env = observer_env()
    key = _create_observer(env, "bad-media-observer")
    valid_data = _silent_flac_bytes()

    response = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": DAY,
            "segment": SEGMENT,
            "files": [
                (io.BytesIO(b""), "empty.flac"),
                (io.BytesIO(valid_data), "audio.flac"),
            ],
        },
        environ_overrides={"pl.identity": _pl_identity()},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["files"] == ["audio.flac"]
    assert data["bytes"] == len(valid_data)
    segment = env.journal / "chronicle" / DAY / "bad-media-observer" / SEGMENT
    assert (segment / "audio.flac").read_bytes() == valid_data
    assert not (segment / "empty.flac").exists()


def test_ac3_silent_audio_records_empty_no_stt(segment_journal, monkeypatch):
    segment = _segment_dir(segment_journal)
    preserve_audio = segment / "audio.flac"
    _write_silent_flac(preserve_audio)

    record, stt_spy, jsonl_path = _drive_transcribe(
        monkeypatch,
        preserve_audio,
        preserve_all=True,
    )

    assert record is not None
    _assert_processing_record(
        record,
        state=STATE_EMPTY,
        reason_code=REASON_NO_DECODABLE_AUDIO,
    )
    assert stt_spy.call_count == 0
    assert len(jsonl_path.read_text(encoding="utf-8").splitlines()) == 1

    filter_audio = segment / "filtered.flac"
    _write_silent_flac(filter_audio)
    filtered_record, filtered_stt, filtered_jsonl = _drive_transcribe(
        monkeypatch,
        filter_audio,
        preserve_all=False,
    )

    assert filtered_record is not None
    _assert_processing_record(
        filtered_record,
        state=STATE_EMPTY,
        reason_code=REASON_NO_DECODABLE_AUDIO,
    )
    assert filtered_stt.call_count == 0
    assert not filter_audio.exists()
    assert filtered_jsonl.exists()
    assert len(filtered_jsonl.read_text(encoding="utf-8").splitlines()) == 1


def test_corrupt_audio_decode_records_failed_without_vad_or_stt(
    segment_journal,
    monkeypatch,
):
    from solstone.observe import processing_record

    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    vad_module = importlib.import_module("solstone.observe.vad")
    segment = _segment_dir(segment_journal)
    audio_path = segment / "audio.m4a"
    audio_path.write_bytes(b"truncated")

    stt_spy = Mock(return_value=[])
    vad_spy = Mock()
    monkeypatch.setattr(processing_record, "now_iso_utc", lambda: FIXED_NOW)
    monkeypatch.setattr(transcribe_main, "callosum_send", lambda *args, **kwargs: None)
    load_audio_spy = Mock(side_effect=AudioDecodeError("worker exited from signal 11"))
    monkeypatch.setattr(transcribe_main, "load_audio", load_audio_spy)
    monkeypatch.setattr(transcribe_main, "stt_transcribe", stt_spy)
    monkeypatch.setattr(vad_module, "run_vad", vad_spy)

    transcribe_main._process_one(
        audio_path,
        argparse.Namespace(backend=None, cpu=False, model=None, redo=False),
        {"preserve_all": True},
        "parakeet",
    )

    jsonl_path = audio_path.with_suffix(".jsonl")
    record = _read_processing_record(jsonl_path)
    _assert_processing_record(
        record,
        state=STATE_FAILED,
        reason_code=REASON_CORRUPT_INPUT,
    )
    assert len(jsonl_path.read_text(encoding="utf-8").splitlines()) == 1
    assert stt_spy.call_count == 0
    assert vad_spy.call_count == 0
    assert load_audio_spy.call_count == 1
    assert read_segment_data_state(DAY, SEGMENT) == {
        "audio": DataState.FAILED_FINAL.value
    }

    load_audio_spy.reset_mock()
    stt_spy.reset_mock()
    vad_spy.reset_mock()
    transcribe_main._process_one(
        audio_path,
        argparse.Namespace(backend=None, cpu=False, model=None, redo=False),
        {"preserve_all": True},
        "parakeet",
    )

    assert load_audio_spy.call_count == 0
    assert stt_spy.call_count == 0
    assert vad_spy.call_count == 0


def test_ac9_corrupt_output_jsonl_derives_pending(segment_journal):
    segment = _segment_dir(segment_journal)
    (segment / "screen.jsonl").write_bytes(b"\x00\x01 not json\n")

    assert read_segment_data_state(DAY, SEGMENT) == {"screen": DataState.PENDING.value}
    completion = classify_segment_completion(
        cluster_segments(DAY),
        read_segment_progress(DAY),
    )
    assert completion.blockers == [
        {
            "segment": SEGMENT,
            "dimension": "not_sensed",
            "detail": f"screen={DataState.PENDING.value}",
        }
    ]
