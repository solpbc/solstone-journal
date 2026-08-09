# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the native speakers-analyze adapter."""

from __future__ import annotations

import json
import stat
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from solstone.apps.speakers.encoder_config import (
    ENCODER_ID,
    SPEAKERS_ANALYZE_DTYPE,
    SPEAKERS_ANALYZE_PAYLOAD_FORMAT,
    WESPEAKER_EMBEDDING_WIDTH,
)
from solstone.observe.transcribe.speakers_analyze_adapter import (
    RESPONSE_SCHEMA,
    TEMP_PREFIX,
    HelperInvocationResult,
    analyze_speakers,
    sweep_stale_speakers_analyze_dirs,
)
from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError

_DEFAULT_LABELS = object()


def _statements() -> list[dict[str, Any]]:
    return [
        {"id": 1, "start": 0.0, "end": 0.5, "text": "one"},
        {"id": 2, "start": 0.6, "end": 1.1, "text": "two"},
    ]


def _response(
    *,
    statement_ids: list[int] | None = None,
    durations_s: list[float] | None = None,
    labels: object = _DEFAULT_LABELS,
    rows: int | None = None,
    byte_count: int | None = None,
    shape: list[int] | None = None,
    speaker_evidence: str = "multi",
    overlap_fraction: float = 0.25,
    multi_window_fraction: float = 0.5,
    mean_window_overlap_share: float = 0.125,
) -> dict[str, Any]:
    statement_ids = [1, 2] if statement_ids is None else statement_ids
    rows = len(statement_ids) if rows is None else rows
    expected_bytes = rows * WESPEAKER_EMBEDDING_WIDTH * 4
    return {
        "schema": RESPONSE_SCHEMA,
        "sample_rate_hz": 10,
        "inputs": {
            "statement_embedding": {
                "statement_ids": [1, 2],
                "spans_s": [[0.0, 0.5], [0.6, 1.1]],
            },
            "diarization": {
                "statement_ids": [1, 2],
                "spans_s": [[0.0, 0.5], [0.6, 1.1]],
            },
        },
        "statement_embeddings": {
            "audio_buffer": "full",
            "encoder": ENCODER_ID,
            "payload_format": SPEAKERS_ANALYZE_PAYLOAD_FORMAT,
            "payload_path": "__filled_by_test__",
            "dtype": SPEAKERS_ANALYZE_DTYPE,
            "statement_ids": statement_ids,
            "durations_s": [0.5 for _ in statement_ids]
            if durations_s is None
            else durations_s,
            "shape": [rows, WESPEAKER_EMBEDDING_WIDTH] if shape is None else shape,
            "byte_count": expected_bytes if byte_count is None else byte_count,
            "admitted_count": rows,
            "skipped_count": 2 - rows,
        },
        "pyannote": {
            "window_stats": [
                {"speech_frames": 100, "active_slot_count": 2, "overlap_frames": 10}
            ]
        },
        "evidence": {
            "overlap_fraction": overlap_fraction,
            "speaker_evidence": speaker_evidence,
            "multi_window_fraction": multi_window_fraction,
            "mean_window_overlap_share": mean_window_overlap_share,
        },
        "diarization": {
            "intervals": [],
            "valid_intervals": None,
            "interval_embeddings": None,
            "cluster_labels": None,
            "statement_labels": [2, None] if labels is _DEFAULT_LABELS else labels,
            "silhouette_k": None,
            "effective_k": None,
        },
    }


def _payload(rows: int, *, nonfinite: bool = False) -> bytes:
    values = np.arange(rows * WESPEAKER_EMBEDDING_WIDTH, dtype="<f4")
    if nonfinite and len(values):
        values[0] = np.inf
    return values.tobytes()


def _run_adapter(
    tmp_path: Path,
    *,
    response: dict[str, Any] | None = None,
    payload: bytes | None = None,
    returncode: int = 0,
    stdout: str | None = None,
    stderr: str = "",
    statements_pre_restore: list[dict[str, Any]] | None = None,
    statements_restored: list[dict[str, Any]] | None = None,
) -> tuple[Any, dict[str, Any], Path]:
    captured: dict[str, Any] = {}
    temp_dirs: list[Path] = []

    def temp_dir_factory(_raw_path: Path) -> Path:
        path = tmp_path / f"adapter-temp-{len(temp_dirs)}"
        path.mkdir(mode=0o700)
        temp_dirs.append(path)
        return path

    def helper_invoker(_argv: list[str], stdin: str, _raw_path: Path):
        request = json.loads(stdin)
        captured["request"] = request
        if returncode != 0:
            return HelperInvocationResult(returncode, stdout or "", stderr)
        if stdout is not None:
            return HelperInvocationResult(returncode, stdout, stderr)
        out = response or _response()
        out["statement_embeddings"]["payload_path"] = request[
            "output_payload_f32le_path"
        ]
        rows = len(out["statement_embeddings"]["statement_ids"])
        Path(request["output_payload_f32le_path"]).write_bytes(
            _payload(rows) if payload is None else payload
        )
        return HelperInvocationResult(0, json.dumps(out), "")

    result = analyze_speakers(
        raw_path=tmp_path
        / "chronicle"
        / "20260101"
        / "mic"
        / "090000_300"
        / "mic_audio.flac",
        full_audio=np.arange(20, dtype=np.float32),
        statement_audio=np.arange(20, dtype=np.float32),
        reduced_audio=np.arange(10, dtype=np.float32),
        statements_pre_restore=statements_pre_restore or _statements(),
        statements_restored=statements_restored
        or [
            {"id": 1, "start": 10.0, "end": 10.5, "text": "one"},
            {"id": 2, "start": 11.0, "end": 11.5, "text": "two"},
        ],
        sample_rate=10,
        min_statement_duration=0.3,
        helper_locator=lambda: tmp_path / "helper",
        helper_invoker=helper_invoker,
        model_path_resolver=lambda: (tmp_path / "wespeaker.onnx", tmp_path / "p.onnx"),
        temp_dir_factory=temp_dir_factory,
    )
    return result, captured["request"], temp_dirs[0]


def test_success_maps_request_response_payload_and_cleans_temp_dir(tmp_path: Path):
    result, request, temp_dir = _run_adapter(tmp_path)

    assert not temp_dir.exists()
    assert request["interval_embedding_payload_f32le_path"] is None
    assert request["statement_embedding"]["spans"][0]["start_s"] == 0.0
    assert request["diarization"]["spans"][0]["start_s"] == 10.0
    assert result.statements == [
        {"id": 1, "start": 10.0, "end": 10.5, "text": "one", "speaker": 2},
        {"id": 2, "start": 11.0, "end": 11.5, "text": "two"},
    ]
    assert result.embedding_payload is not None
    assert result.embedding_payload.statement_ids == [1, 2]
    assert len(result.embedding_payload.payload) == 2 * WESPEAKER_EMBEDDING_WIDTH * 4
    assert result.speaker_evidence.speaker_evidence == "multi"
    assert result.overlap_fraction == 0.25


def test_helper_shaped_wire_literals_are_accepted(tmp_path: Path):
    response = _response()
    assert (
        response["statement_embeddings"]["payload_format"]
        == SPEAKERS_ANALYZE_PAYLOAD_FORMAT
    )
    assert response["statement_embeddings"]["dtype"] == SPEAKERS_ANALYZE_DTYPE

    result, _request, _temp_dir = _run_adapter(tmp_path, response=response)

    assert result.embedding_payload is not None


@pytest.mark.parametrize(
    ("mutate", "reason"),
    [
        (
            lambda r: r["statement_embeddings"].update(statement_ids=[1, 1]),
            "duplicate-statement-id",
        ),
        (
            lambda r: r["statement_embeddings"].update(statement_ids=[2, 1]),
            "statement-id-divergence",
        ),
        (
            lambda r: r["statement_embeddings"].update(statement_ids=[1, 99]),
            "foreign-statement-id",
        ),
        (
            lambda r: r["statement_embeddings"].update(durations_s=[0.5]),
            "duration-count-mismatch",
        ),
        (
            lambda r: r["statement_embeddings"].update(durations_s=[0.5, float("inf")]),
            "nonfinite-duration",
        ),
        (
            lambda r: r["statement_embeddings"].update(shape=[2, 255]),
            "embedding-shape-mismatch",
        ),
        (
            lambda r: r["statement_embeddings"].update(byte_count=1),
            "embedding-byte-count-mismatch",
        ),
        (
            lambda r: r["statement_embeddings"].update(admitted_count=1),
            "embedding-admitted-count-mismatch",
        ),
        (
            lambda r: r["statement_embeddings"].update(skipped_count=9),
            "embedding-skipped-count-mismatch",
        ),
        (
            lambda r: r["evidence"].update(speaker_evidence="unknown"),
            "unknown-speaker-evidence",
        ),
        (
            lambda r: r["evidence"].update(overlap_fraction=1.1),
            "invalid-overlap-fraction",
        ),
        (
            lambda r: r["evidence"].update(multi_window_fraction=float("nan")),
            "invalid-multi-window-fraction",
        ),
        (
            lambda r: r["diarization"].update(statement_labels=[0, None]),
            "invalid-statement-labels",
        ),
        (
            lambda r: r["diarization"].update(statement_labels=[1]),
            "statement-label-count-mismatch",
        ),
        (
            lambda r: r["statement_embeddings"].update(payload_format="f32le"),
            "invalid-payload-format",
        ),
        (
            lambda r: r["statement_embeddings"].update(dtype="float32"),
            "invalid-dtype",
        ),
    ],
)
def test_response_validation_rejects_invalid_payload_shapes(
    tmp_path: Path, mutate, reason: str
):
    response = _response()
    mutate(response)

    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, response=response)

    assert exc.value.stage == "payload"
    assert exc.value.reason == reason


def test_nonfinite_consumed_embedding_rejected(tmp_path: Path):
    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, payload=_payload(2, nonfinite=True))

    assert exc.value.stage == "payload"
    assert exc.value.reason == "nonfinite-embedding"


def test_payload_size_checked_before_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    response = _response()

    def fail_read_bytes(_path: Path) -> bytes:
        raise AssertionError("payload bytes were read before stat-size validation")

    monkeypatch.setattr(Path, "read_bytes", fail_read_bytes)

    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, response=response, payload=b"oversized")

    assert exc.value.stage == "payload"
    assert exc.value.reason == "embedding-payload-size-mismatch"


def test_unexpected_internal_exception_propagates_and_cleans_temp_dir(tmp_path: Path):
    class UnexpectedAdapterBug(RuntimeError):
        pass

    temp_dir = tmp_path / "adapter-temp"

    def temp_dir_factory(_raw_path: Path) -> Path:
        temp_dir.mkdir(mode=0o700)
        return temp_dir

    def statements_restored() -> list[dict[str, Any]]:
        raise UnexpectedAdapterBug("boom")

    with pytest.raises(UnexpectedAdapterBug, match="boom"):
        analyze_speakers(
            raw_path=tmp_path / "audio.flac",
            full_audio=np.zeros(20, dtype=np.float32),
            statement_audio=np.zeros(20, dtype=np.float32),
            reduced_audio=None,
            statements_pre_restore=[{"id": 1, "start": 0.0, "end": 0.5, "text": "x"}],
            statements_restored=statements_restored,
            sample_rate=10,
            min_statement_duration=0.3,
            helper_locator=lambda: tmp_path / "helper",
            helper_invoker=lambda _argv, _stdin, _raw_path: pytest.fail(
                "helper should not be invoked after restoration fails"
            ),
            model_path_resolver=lambda: (tmp_path / "w.onnx", tmp_path / "p.onnx"),
            temp_dir_factory=temp_dir_factory,
        )

    assert not temp_dir.exists()


def test_gate_decline_null_labels_is_accepted(tmp_path: Path):
    result, _request, _temp_dir = _run_adapter(
        tmp_path,
        response=_response(labels=None, speaker_evidence="single"),
    )

    assert result.statement_labels is None
    assert all("speaker" not in statement for statement in result.statements)


def test_zero_admitted_rows_are_accepted_without_embeddings(tmp_path: Path):
    short_statements = [
        {"id": 1, "start": 0.0, "end": 0.1, "text": "one"},
        {"id": 2, "start": 0.1, "end": 0.2, "text": "two"},
    ]

    result, _request, _temp_dir = _run_adapter(
        tmp_path,
        response=_response(statement_ids=[], labels=None, speaker_evidence="single"),
        payload=b"",
        statements_pre_restore=short_statements,
        statements_restored=short_statements,
    )

    assert result.embedding_payload is None
    assert result.statements == short_statements
    assert result.statement_labels is None


def test_malformed_json_response_maps_to_parse_failure(tmp_path: Path):
    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, stdout="{not-json")

    assert exc.value.stage == "parse"
    assert exc.value.reason == "malformed-response"


def test_signal_returncode_maps_to_invoke_failure(tmp_path: Path):
    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, returncode=-9)

    assert exc.value.stage == "invoke"
    assert exc.value.reason == "signal-9"
    assert exc.value.native_exit_code == -9


def test_helper_error_json_reason_maps_to_invoke_failure(tmp_path: Path):
    stderr = json.dumps(
        {
            "schema": "solstone-speaker-analyze-error-v1",
            "reason": "model-missing",
        }
    )

    with pytest.raises(SpeakerAnalyzeError) as exc:
        _run_adapter(tmp_path, returncode=69, stderr=stderr)

    assert exc.value.stage == "invoke"
    assert exc.value.reason == "model-missing"
    assert exc.value.native_exit_code == 69
    assert (
        exc.value.event_fields()["speaker_analysis_failure_reason"] == "model-missing"
    )


def test_temp_dir_and_files_are_owner_only_and_cleaned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    import solstone.observe.transcribe.speakers_analyze_adapter as adapter

    captured: dict[str, int] = {}
    monkeypatch.setattr(adapter, "TEMP_ROOT", tmp_path)

    def helper_invoker(_argv: list[str], stdin: str, _raw_path: Path):
        request = json.loads(stdin)
        full_path = Path(request["full_audio_f32le_path"])
        reduced_path = Path(request["reduced_audio_f32le_path"])
        captured["dir_mode"] = stat.S_IMODE(full_path.parent.stat().st_mode)
        captured["full_file_mode"] = stat.S_IMODE(full_path.stat().st_mode)
        captured["reduced_file_mode"] = stat.S_IMODE(reduced_path.stat().st_mode)
        response = _response()
        response["statement_embeddings"]["payload_path"] = request[
            "output_payload_f32le_path"
        ]
        Path(request["output_payload_f32le_path"]).write_bytes(_payload(2))
        return HelperInvocationResult(0, json.dumps(response), "")

    analyze_speakers(
        raw_path=tmp_path
        / "chronicle"
        / "20260101"
        / "mic"
        / "090000_300"
        / "mic_audio.flac",
        full_audio=np.ones(20, dtype=np.float32),
        statement_audio=np.ones(20, dtype=np.float32),
        reduced_audio=np.ones(10, dtype=np.float32),
        statements_pre_restore=_statements(),
        statements_restored=_statements(),
        sample_rate=10,
        min_statement_duration=0.3,
        helper_locator=lambda: tmp_path / "helper",
        helper_invoker=helper_invoker,
        model_path_resolver=lambda: (tmp_path / "wespeaker.onnx", tmp_path / "p.onnx"),
    )

    assert captured == {
        "dir_mode": 0o700,
        "full_file_mode": 0o600,
        "reduced_file_mode": 0o600,
    }
    assert list(tmp_path.glob(f"{TEMP_PREFIX}*")) == []


def test_sweep_stale_speakers_analyze_dirs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    import solstone.observe.transcribe.speakers_analyze_adapter as adapter

    monkeypatch.setattr(adapter, "TEMP_ROOT", tmp_path)
    stale = tmp_path / f"{TEMP_PREFIX}old"
    stale.mkdir()
    fresh = tmp_path / f"{TEMP_PREFIX}fresh"
    fresh.mkdir()
    old_time = 1_600_000_000
    stale.touch()
    fresh.touch()
    monkeypatch.setattr(adapter.time, "time", lambda: old_time + 10_000)
    stale.touch()
    import os

    os.utime(stale, (old_time, old_time))
    os.utime(fresh, (old_time + 9_999, old_time + 9_999))

    assert sweep_stale_speakers_analyze_dirs(max_age_seconds=100) == 1
    assert not stale.exists()
    assert fresh.exists()
