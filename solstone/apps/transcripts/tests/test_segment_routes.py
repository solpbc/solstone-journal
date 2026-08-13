# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import builtins
import datetime as datetime_module
import io
import json
import math
import os
import re
import shutil
import subprocess
import time
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

import av
import numpy as np
import pytest

import solstone.apps.transcripts.routes as routes
from solstone.apps.transcripts.copy import (
    SPEAKER_LABEL_SOURCE_AMBIGUOUS_MESSAGE,
    SPEAKER_LABELS_UNAVAILABLE_MESSAGE,
)
from solstone.apps.transcripts.routes import (
    _attach_streams_to_ranges,
    _segment_modality_signals,
    _timestamp_from_day_time,
    _watch_reprocess_completion,
)
from solstone.apps.transcripts.tests._media_helpers import (
    build_moov_at_tail_m4a,
    head_bytes,
    read_true_duration_seconds,
    top_level_atom_order,
)
from solstone.observe.processing_record import (
    FAILED_ATTEMPT_BOUND,
    STATE_EMPTY,
    STATE_FAILED,
)
from solstone.think.data_state import ANALYZING_STALE_SECONDS
from solstone.think.importers import health_schema
from tests.helpers.module_mocks import module_mock

# 20260304 is the canonical fully-analyzed reference day; see
# tests/fixtures/journal/chronicle/20260304/README.md and
# tests/test_reference_day_fixture.py.
def _retention_executor_or_skip() -> str:
    """The real retention executor, or skip.

    ⚠ Deliberately the REAL binary rather than a fake. A fake that mimicked the
    executor would be an oracle this repository controls, and what these tests need to
    know is what the actual removal does to the actual journal -- that the raw is
    gone, that a tombstone remains as the owner's evidence, and that the segment stops
    appearing in the listing. When the binary is not built, that is worth SAYING rather
    than silently asserting against a stand-in.
    """
    import os
    import shutil as _shutil

    override = os.environ.get("SOLSTONE_RETENTION_BIN")
    if override and os.access(override, os.X_OK):
        return override
    found = _shutil.which("solstone-retention")
    if found:
        return found
    for profile in ("debug", "release"):
        candidate = (
            Path(__file__).resolve().parents[4]
            / "core"
            / "target"
            / profile
            / "solstone-retention"
        )
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    pytest.skip(
        "solstone-retention is not built; every removal of the owner's media goes "
        "through it, so this test has nothing real to assert against "
        "(build it with: cargo build -p solstone-core-retention-cli)"
    )


@pytest.fixture
def retention_executor_available(monkeypatch):
    """Make the real executor reachable from the route under test."""
    monkeypatch.setenv("SOLSTONE_RETENTION_BIN", _retention_executor_or_skip())


FIXTURE_DAY = "20260304"
FIXTURE_STREAM = "default"
FIXTURE_SEGMENT = "090000_300"
_requires_ffmpeg = pytest.mark.skipif(
    shutil.which("ffmpeg") is None or shutil.which("ffprobe") is None,
    reason="ffmpeg/ffprobe not installed",
)


@pytest.fixture(autouse=True)
def _isolate_routes_stdlib_modules(monkeypatch):
    monkeypatch.setattr(routes, "subprocess", module_mock(routes.subprocess))
    monkeypatch.setattr(routes, "threading", module_mock(routes.threading))


def _apple_health_card_stream() -> str:
    return health_schema.health_card_stream(health_schema.SOURCE_APPLE_HEALTH)


class _FakeDeferredDeletes:
    def __init__(self) -> None:
        self.scheduled: list[tuple[str, Callable[[], None], float]] = []
        self._pending: dict[str, tuple[Callable[[], None], float]] = {}

    def schedule_with_id(
        self,
        pending_id: str,
        commit_fn: Callable[[], None],
        ttl_seconds: float = 10.0,
    ) -> str:
        self.scheduled.append((pending_id, commit_fn, ttl_seconds))
        self._pending[pending_id] = (commit_fn, ttl_seconds)
        return pending_id

    def cancel(self, pending_id: str) -> bool:
        if pending_id not in self._pending:
            return False
        self._pending.pop(pending_id)
        return True

    def fire(self, pending_id: str) -> None:
        commit_fn, _ttl_seconds = self._pending.pop(pending_id)
        commit_fn()


@pytest.fixture
def fake_deferred_deletes(monkeypatch):
    fake = _FakeDeferredDeletes()
    monkeypatch.setattr(routes, "deferred_deletes", fake)
    return fake


def _assert_reason(response, *, error: str, reason_code: str, detail: str) -> None:
    payload = response.get_json()
    assert payload["error"] == error
    assert payload["reason_code"] == reason_code
    assert payload["detail"] == detail


def _write_segment(
    journal_root,
    day: str,
    stream: str,
    segment: str,
    *,
    audio: bool = True,
    screen: bool = True,
    audio_state: str = "analyzed",
    screen_state: str = "analyzed",
) -> None:
    segment_dir = journal_root / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    if audio:
        audio_entries = [{"raw": "audio.flac"}]
        if audio_state == "analyzed":
            audio_entries.append(
                {"start": "00:00:01", "source": "mic", "text": "audio line"}
            )
        _write_jsonl(segment_dir / "audio.jsonl", audio_entries)
    if screen:
        screen_entries = [{"raw": "screen.webm"}]
        if screen_state == "analyzed":
            screen_entries.append(
                {
                    "frame_id": 1,
                    "timestamp": 1,
                    "analysis": {"primary": "work"},
                }
            )
        _write_jsonl(segment_dir / "screen.jsonl", screen_entries)


def _write_jsonl(path, entries: list[dict]) -> None:
    path.write_text(
        "\n".join(json.dumps(entry) for entry in entries) + "\n",
        encoding="utf-8",
    )


def _write_speaker_labels(segment_dir: Path, labels: list[dict]) -> None:
    labels_dir = segment_dir / "talents"
    labels_dir.mkdir(parents=True, exist_ok=True)
    (labels_dir / "speaker_labels.json").write_text(
        json.dumps(
            {
                "labels": labels,
                "owner_centroid_last_refreshed_at": "test",
                "voiceprint_versions": {},
            }
        )
        + "\n",
        encoding="utf-8",
    )


def _write_embedding_npz(
    segment_dir: Path,
    *,
    source: str = "audio",
    statement_ids: tuple[int, ...] = (1, 4),
) -> None:
    embeddings = np.ones((len(statement_ids), 256), dtype=np.float32)
    np.savez_compressed(
        segment_dir / f"{source}.npz",
        embeddings=embeddings,
        statement_ids=np.array(statement_ids, dtype=np.int64),
        durations_s=np.ones(len(statement_ids), dtype=np.float32),
    )


def _empty_screen_header(raw: str = "screen.webm") -> dict:
    return {"raw": raw, "_solstone_processing": {"state": STATE_EMPTY}}


def _segment_event(
    event: str,
    segment: str,
    name: str | None = None,
    ts: int = 1,
    **extra,
) -> dict:
    record = {"event": event, "ts": ts, "mode": "segment", "segment": segment}
    if name is not None:
        record["name"] = name
    record.update(extra)
    return record


def _dispatch(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.dispatch", segment, name, ts)


def _complete(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.complete", segment, name, ts, state="finish")


def _fail(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.fail", segment, name, ts, state="error")


def _sense_complete(segment: str, density: str = "active", ts: int = 1) -> dict:
    return _segment_event("sense.complete", segment, ts=ts, density=density)


def _complete_segment_events(segment: str) -> list[dict]:
    return [
        _dispatch(segment, "sense", 10),
        _complete(segment, "sense", 11),
        _sense_complete(segment, "active", 12),
        _dispatch(segment, "entities", 13),
        _complete(segment, "entities", 14),
        _dispatch(segment, "documents", 15),
        _complete(segment, "documents", 16),
    ]


def _write_health(journal_root, day: str, filename: str, entries: list[dict]) -> None:
    path = journal_root / "chronicle" / day / "health" / filename
    path.parent.mkdir(parents=True, exist_ok=True)
    _write_jsonl(path, entries)


def _write_raw_pending_segment(
    journal_root,
    day: str,
    stream: str,
    segment: str,
    *,
    audio: bool = False,
    screen: bool = True,
) -> Path:
    segment_dir = journal_root / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    if audio:
        (segment_dir / "audio.flac").write_bytes(b"audio")
        _write_jsonl(segment_dir / "audio.jsonl", [{"raw": "audio.flac"}])
    if screen:
        (segment_dir / "screen.webm").write_bytes(b"screen")
        _write_jsonl(segment_dir / "screen.jsonl", [{"raw": "screen.webm"}])
    return segment_dir


def _write_analyzing_marker(
    segment_dir: Path,
    *,
    modality: str = "screen",
    request_id: str = "req-1",
) -> Path:
    marker = segment_dir / f".analyzing_{modality}"
    marker.write_text(
        json.dumps(
            {
                "started_at": "2026-05-20T09:00:00Z",
                "modality": modality,
                "request_id": request_id,
            }
        )
        + "\n",
        encoding="utf-8",
    )
    return marker


class _ProcStub:
    def __init__(self, rc: int = 0, stderr: bytes = b"") -> None:
        self.rc = rc
        self.stderr = io.BytesIO(stderr)
        self.stdout = io.BytesIO()

    def wait(self) -> int:
        return self.rc


class _ThreadStub:
    def __init__(self, *, target, args, daemon) -> None:
        self.target = target
        self.args = args
        self.daemon = daemon
        self.started = False

    def start(self) -> None:
        self.started = True


def _stub_reprocess_spawn(monkeypatch, proc: _ProcStub | None = None):
    popen_calls = []
    threads = []

    def fake_popen(argv, **kwargs):
        popen_calls.append((argv, kwargs))
        return proc or _ProcStub()

    def fake_thread(*, target, args, daemon):
        thread = _ThreadStub(target=target, args=args, daemon=daemon)
        threads.append(thread)
        return thread

    monkeypatch.setattr("solstone.apps.transcripts.routes.subprocess.Popen", fake_popen)
    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.threading.Thread", fake_thread
    )
    return popen_calls, threads


def _write_moov_tail_audio_segment(
    journal_root,
    tmp_path,
    day: str,
    stream: str,
    segment: str,
    duration_seconds: float,
) -> tuple[Path, float]:
    _write_segment(journal_root, day, stream, segment, screen=False)
    source_path = tmp_path / f"{day}-{segment}-raw.m4a"
    build_moov_at_tail_m4a(source_path, duration_seconds)
    true_duration = read_true_duration_seconds(source_path)

    segment_dir = journal_root / "chronicle" / day / stream / segment
    raw_path = segment_dir / "raw.m4a"
    shutil.copyfile(source_path, raw_path)
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "raw.m4a", "duration": true_duration},
            {
                "start": "00:00:01",
                "source": "mic",
                "speaker": 1,
                "text": "tail moov duration",
            },
        ],
    )
    return raw_path, true_duration


def _action_log_rows(journal_root, day):
    log_path = journal_root / "config" / "actions" / f"{day}.jsonl"
    if not log_path.exists():
        return []
    return [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def test_ranges_returns_object_shape_with_streams(client, journal_copy):
    day = "20990102"
    _write_segment(journal_copy, day, "alpha", "090000_300")
    _write_segment(journal_copy, day, "bravo", "090500_300")
    _write_segment(journal_copy, day, "alpha", "091000_300")

    response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert response.status_code == 200
    data = response.get_json()
    assert set(data) == {"audio", "screen"}
    assert data["audio"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["alpha", "bravo"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]
    assert data["screen"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["alpha", "bravo"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]


def test_ranges_overflow_returns_full_list(client, journal_copy):
    day = "20990103"
    for stream in ["echo", "alpha", "delta", "bravo", "charlie"]:
        _write_segment(journal_copy, day, stream, "090000_300", screen=False)

    response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert response.status_code == 200
    assert response.get_json()["audio"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["alpha", "bravo", "charlie", "delta", "echo"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]


def test_ranges_single_stream(client, journal_copy):
    day = "20990104"
    _write_segment(journal_copy, day, "solo", "090000_300", screen=False)

    response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert response.status_code == 200
    assert response.get_json()["audio"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["solo"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]


def test_day_returns_object_shape_with_streams(client, journal_copy):
    day = "20990105"
    _write_segment(journal_copy, day, "alpha", "090000_300")
    _write_segment(journal_copy, day, "bravo", "090500_300", screen=False)

    response = client.get(f"/app/transcripts/api/day/{day}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["audio"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["alpha", "bravo"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]
    assert data["screen"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["alpha"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]
    assert data["segments"] == [
        {
            "key": "090000_300",
            "start": "09:00",
            "end": "09:05",
            "types": ["audio", "screen"],
            "stream": "alpha",
            "data_state": {"audio": "analyzed", "screen": "analyzed"},
            "think": "awaiting",
        },
        {
            "key": "090500_300",
            "start": "09:05",
            "end": "09:10",
            "types": ["audio"],
            "stream": "bravo",
            "data_state": {"audio": "analyzed"},
            "think": "awaiting",
        },
    ]


def test_attach_streams_to_ranges_empty_when_no_overlap():
    result = _attach_streams_to_ranges([("09:00", "09:15")], [], "audio")

    assert result == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": [],
            "state": "pending",
            "think": None,
        }
    ]


def test_routes_expose_sense_and_think_axes(client, journal_copy):
    day = "20990116"
    thought_segment = "090000_300"
    awaiting_segment = "090500_300"
    recovered_segment = "091000_300"
    for segment in (thought_segment, awaiting_segment, recovered_segment):
        _write_segment(journal_copy, day, "default", segment)
    _write_health(
        journal_copy,
        day,
        "001_segment.jsonl",
        _complete_segment_events(thought_segment)
        + [_sense_complete(awaiting_segment, "active", 30)]
        + [
            _dispatch(recovered_segment, "sense", 40),
            _complete(recovered_segment, "sense", 41),
            _sense_complete(recovered_segment, "active", 42),
            _dispatch(recovered_segment, "entities", 43),
            _fail(recovered_segment, "entities", 44),
            _complete(recovered_segment, "entities", 45),
            _dispatch(recovered_segment, "documents", 46),
            _complete(recovered_segment, "documents", 47),
        ],
    )

    day_response = client.get(f"/app/transcripts/api/day/{day}")
    ranges_response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert day_response.status_code == 200
    segments = {seg["key"]: seg for seg in day_response.get_json()["segments"]}
    assert segments[thought_segment]["think"] == "thought"
    assert segments[awaiting_segment]["think"] == "awaiting"
    assert segments[thought_segment]["think"] != segments[awaiting_segment]["think"]
    assert segments[recovered_segment]["think"] == "thought"

    assert ranges_response.status_code == 200
    ranges = ranges_response.get_json()
    assert ranges["audio"][0]["state"] == "analyzed"
    assert ranges["audio"][0]["think"] == "awaiting"
    assert ranges["screen"][0]["think"] == "awaiting"


def test_ranges_best_state_wins_for_mixed_pending_and_analyzed(client, journal_copy):
    day = "20990109"
    _write_segment(
        journal_copy,
        day,
        "default",
        "090000_300",
        audio=False,
        screen_state="pending",
    )
    _write_segment(
        journal_copy,
        day,
        "default",
        "090500_300",
        audio=False,
        screen_state="analyzed",
    )

    response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert response.status_code == 200
    assert response.get_json()["screen"] == [
        {
            "start": "09:00",
            "end": "09:15",
            "streams": ["default"],
            "state": "analyzed",
            "think": "awaiting",
        }
    ]


@pytest.mark.parametrize("stream", ["-bad", "Upper", "..bad"])
def test_segment_content_rejects_invalid_stream(client, stream):
    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{stream}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 404
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Invalid stream format",
    )


@pytest.mark.parametrize("stream", ["-bad", "Upper", "..bad"])
def test_delete_segment_rejects_invalid_stream(client, stream):
    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{stream}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Invalid stream format",
    )


def test_segment_content_missing_segment_does_not_create_phantom_directory(
    client, journal_copy
):
    response = client.get("/app/transcripts/api/segment/29990101/default/090000_300")

    assert response.status_code == 404
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Segment directory not found",
    )
    assert not (journal_copy / "chronicle" / "29990101").exists()
    assert not (
        journal_copy / "chronicle" / "29990101" / "default" / "090000_300"
    ).exists()


def test_delete_missing_segment_does_not_create_phantom_directory(client, journal_copy):
    response = client.delete("/app/transcripts/api/segment/29990101/default/090000_300")

    assert response.status_code == 404
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Segment not found",
    )
    assert not (journal_copy / "chronicle" / "29990101").exists()
    assert not (
        journal_copy / "chronicle" / "29990101" / "default" / "090000_300"
    ).exists()


def test_segment_content_happy_path_returns_segment_payload(client):
    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["segment_key"] == FIXTURE_SEGMENT
    assert data["chunks"]
    assert "media_sizes" in data
    assert data["data_state"] == {"audio": "analyzed", "screen": "analyzed"}
    assert set(data["media_purged"]) == {"audio", "screen"}
    assert all(isinstance(value, bool) for value in data["media_purged"].values())


def test_segment_content_adds_speaker_provenance_without_embeddings(client):
    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"] == {
        "present": True,
        "loaded": True,
        "source": "audio",
        "ambiguous": False,
    }
    assert "TR_SPEAKER_UNKNOWN_CHIP" in data["transcripts_copy"]
    audio_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "audio"]
    assert [chunk["sentence_id"] for chunk in audio_chunks] == [1, 2, 3, 4, 5]
    assert all(chunk["speaker_source"] == "audio" for chunk in audio_chunks)
    assert all(chunk["source_ref"]["source"] == "mic" for chunk in audio_chunks)
    assert all(chunk["has_embedding"] is False for chunk in audio_chunks)
    assert all(chunk["speaker_actionable"] is False for chunk in audio_chunks)
    assert audio_chunks[0]["speaker_label"] == {
        "name": "Romeo Montague",
        "entity_id": "romeo_montague",
        "confidence": "high",
        "confidence_state": "high",
        "method": "owner_centroid",
        "owner_margin_declined": False,
        "acoustic_margin_declined": False,
        "is_owner": True,
    }
    assert audio_chunks[3]["speaker_label"]["confidence"] == "medium"
    assert audio_chunks[3]["speaker_label"]["confidence_state"] == "medium"


def test_segment_content_marks_seeded_embedding_statement_ids(client, journal_copy):
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )
    _write_embedding_npz(segment_dir, statement_ids=(1, 4))

    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"]["source"] == "audio"
    audio_by_sid = {
        chunk["sentence_id"]: chunk
        for chunk in data["chunks"]
        if chunk["type"] == "audio"
    }
    assert audio_by_sid[1]["has_embedding"] is True
    assert audio_by_sid[1]["speaker_actionable"] is True
    assert audio_by_sid[2]["has_embedding"] is False
    assert audio_by_sid[2]["speaker_actionable"] is False
    assert audio_by_sid[4]["has_embedding"] is True
    assert audio_by_sid[4]["speaker_actionable"] is True
    assert audio_by_sid[5]["has_embedding"] is False
    assert audio_by_sid[5]["speaker_actionable"] is False


def test_segment_content_keeps_missing_confidence_label_as_unknown(
    client,
    journal_copy,
):
    labels_path = (
        journal_copy
        / "chronicle"
        / FIXTURE_DAY
        / FIXTURE_STREAM
        / FIXTURE_SEGMENT
        / "talents"
        / "speaker_labels.json"
    )
    payload = json.loads(labels_path.read_text(encoding="utf-8"))
    payload["labels"][0]["confidence"] = ""
    labels_path.write_text(json.dumps(payload) + "\n", encoding="utf-8")

    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    audio_chunks = [
        chunk for chunk in response.get_json()["chunks"] if chunk["type"] == "audio"
    ]
    assert audio_chunks[0]["speaker_label"]["name"] == "Romeo Montague"
    assert audio_chunks[0]["speaker_label"]["confidence"] == ""
    assert audio_chunks[0]["speaker_label"]["confidence_state"] == "unknown"


def test_segment_content_malformed_speaker_labels_warns(client, journal_copy):
    labels_path = (
        journal_copy
        / "chronicle"
        / FIXTURE_DAY
        / FIXTURE_STREAM
        / FIXTURE_SEGMENT
        / "talents"
        / "speaker_labels.json"
    )
    labels_path.write_text("{bad json\n", encoding="utf-8")

    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"] == {
        "present": True,
        "loaded": False,
        "source": "audio",
        "ambiguous": False,
    }
    assert any(
        detail["type"] == "speaker_labels"
        and detail["file"] == str(labels_path)
        and detail["message"] == SPEAKER_LABELS_UNAVAILABLE_MESSAGE
        for detail in data["warning_details"]
    )
    assert all(
        "speaker_label" not in chunk
        for chunk in data["chunks"]
        if chunk["type"] == "audio"
    )
    assert all(
        chunk["speaker_actionable"] is False
        for chunk in data["chunks"]
        if chunk["type"] == "audio"
    )


def test_segment_content_structurally_bad_speaker_labels_are_not_loaded(
    client,
    journal_copy,
):
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )
    _write_embedding_npz(segment_dir, statement_ids=(1,))
    labels_path = segment_dir / "talents" / "speaker_labels.json"
    labels_path.write_text(
        json.dumps({"labels": {}, "owner_centroid_last_refreshed_at": "test"}) + "\n",
        encoding="utf-8",
    )

    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"] == {
        "present": True,
        "loaded": False,
        "source": "audio",
        "ambiguous": False,
    }
    assert any(
        detail["type"] == "speaker_labels"
        and detail["file"] == str(labels_path)
        and detail["message"] == SPEAKER_LABELS_UNAVAILABLE_MESSAGE
        for detail in data["warning_details"]
    )
    audio_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "audio"]
    assert audio_chunks[0]["has_embedding"] is True
    assert audio_chunks[0]["speaker_actionable"] is False


def test_segment_content_ambiguous_audio_sources_do_not_join_labels(
    client,
    journal_copy,
):
    day = "20990106"
    stream = "default"
    segment = "091000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "mic_audio.jsonl",
        [
            {"raw": "mic_audio.flac"},
            {
                "start": "00:00:02",
                "source": "mic",
                "speaker": 2,
                "text": "mic source line",
            },
        ],
    )
    _write_speaker_labels(
        segment_dir,
        [
            {
                "sentence_id": 1,
                "speaker": "romeo_montague",
                "confidence": "high",
                "method": "owner_centroid",
            }
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"] == {
        "present": True,
        "loaded": True,
        "source": None,
        "ambiguous": True,
    }
    assert any(
        detail["type"] == "speaker_labels"
        and detail["message"] == SPEAKER_LABEL_SOURCE_AMBIGUOUS_MESSAGE
        for detail in data["warning_details"]
    )
    audio_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "audio"]
    assert {chunk["speaker_source"] for chunk in audio_chunks} == {
        "audio",
        "mic_audio",
    }
    assert all(chunk["has_embedding"] is False for chunk in audio_chunks)
    assert all(chunk["speaker_actionable"] is False for chunk in audio_chunks)
    assert all("speaker_label" not in chunk for chunk in audio_chunks)


def test_segment_content_only_labels_source_is_actionable_with_multiple_npz(
    client,
    journal_copy,
):
    day = "20990107"
    stream = "default"
    segment = "091000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "mic_audio.jsonl",
        [
            {"raw": "mic_audio.flac"},
            {
                "start": "00:00:02",
                "source": "mic",
                "speaker": 2,
                "text": "mic source line",
            },
        ],
    )
    _write_embedding_npz(segment_dir, source="audio", statement_ids=(1,))
    _write_embedding_npz(segment_dir, source="mic_audio", statement_ids=(1,))
    _write_speaker_labels(
        segment_dir,
        [
            {
                "sentence_id": 1,
                "speaker": "romeo_montague",
                "confidence": "high",
                "method": "owner_centroid",
            }
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"] == {
        "present": True,
        "loaded": True,
        "source": "audio",
        "ambiguous": False,
    }
    audio_by_source = {
        chunk["speaker_source"]: chunk
        for chunk in data["chunks"]
        if chunk["type"] == "audio"
    }
    assert audio_by_source["audio"]["has_embedding"] is True
    assert audio_by_source["audio"]["speaker_actionable"] is True
    assert audio_by_source["audio"]["speaker_label"]["entity_id"] == "romeo_montague"
    assert audio_by_source["mic_audio"]["has_embedding"] is True
    assert audio_by_source["mic_audio"]["speaker_actionable"] is False
    assert "speaker_label" not in audio_by_source["mic_audio"]


def test_segment_content_invalid_embedding_npz_is_not_route_error(
    client,
    journal_copy,
):
    day = "20990106"
    stream = "default"
    segment = "092000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    (segment_dir / "audio.npz").write_bytes(b"")
    _write_speaker_labels(
        segment_dir,
        [
            {
                "sentence_id": 1,
                "speaker": "romeo_montague",
                "confidence": "high",
                "method": "owner_centroid",
            }
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["speaker_labels"]["source"] == "audio"
    audio_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "audio"]
    assert audio_chunks[0]["speaker_label"]["entity_id"] == "romeo_montague"
    assert audio_chunks[0]["has_embedding"] is False
    assert audio_chunks[0]["speaker_actionable"] is False


def test_segment_content_merges_browser_between_audio_chunks(
    client,
    journal_copy,
    seed_browser_fixture_inventory,
):
    seed_browser_fixture_inventory()
    browser_path = (
        journal_copy
        / "chronicle"
        / "20260702"
        / "workstation.browser"
        / "093000_300"
        / "browser_docs-example-com.jsonl"
    )
    rows = [
        json.loads(line)
        for line in browser_path.read_text(encoding="utf-8").splitlines()
    ]
    # format_audio anchors offsets to the segment's naive local-tz base.
    base = int(datetime_module.datetime(2026, 7, 2, 9, 30, 0).timestamp() * 1000)
    rows[0]["ts"] = base + 30000
    rows[1]["ts"] = base + 35000
    browser_path.write_text(
        "\n".join(json.dumps(row) for row in rows) + "\n",
        encoding="utf-8",
    )

    response = client.get(
        "/app/transcripts/api/segment/20260702/workstation.browser/093000_300"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert [chunk["type"] for chunk in data["chunks"]] == [
        "audio",
        "browser",
        "browser",
        "audio",
    ]
    ts = [chunk["timestamp"] for chunk in data["chunks"]]
    assert ts == sorted(ts)
    assert len(set(ts)) == 4
    assert ts[0] < ts[1] < ts[2] < ts[3]
    assert data["data_state"]["browser"] == "analyzed"


def test_segment_content_browser_only_reads_multiple_site_files(
    client,
    seed_browser_fixture_inventory,
):
    seed_browser_fixture_inventory()

    response = client.get(
        "/app/transcripts/api/segment/20260703/suze.browser/000141_317"
    )

    assert response.status_code == 200
    data = response.get_json()
    browser_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "browser"]
    site_names = {chunk["source_ref"]["site_name"] for chunk in browser_chunks}
    assert {"Gmail", "Docs"} <= site_names
    assert data["data_state"]["browser"] == "analyzed"


def test_segment_content_browser_corrupt_file_warns_and_keeps_valid_site(
    client,
    seed_browser_fixture_inventory,
):
    seed_browser_fixture_inventory()

    response = client.get(
        "/app/transcripts/api/segment/20260701/workstation.browser/100000_300"
    )

    assert response.status_code == 200
    data = response.get_json()
    browser_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "browser"]
    assert data["warnings"] >= 1
    assert any(detail["type"] == "browser" for detail in data["warning_details"])
    assert any(
        chunk["source_ref"]["site_name"] == "valid.example.com"
        for chunk in browser_chunks
    )
    assert data["data_state"]["browser"] == "analyzed"


def test_browser_only_segment_list_think_verdict_is_not_awaiting(
    client,
    seed_browser_fixture_inventory,
):
    seed_browser_fixture_inventory()

    response = client.get("/app/transcripts/api/segments/20260703")

    assert response.status_code == 200
    segments = response.get_json()["segments"]
    browser_segment = next(
        segment
        for segment in segments
        if segment["stream"] == "suze.browser" and segment["key"] == "000141_317"
    )
    assert browser_segment["data_state"]["browser"] == "analyzed"
    assert browser_segment["think"] not in {"awaiting", "stuck"}


def test_markdown_only_import_segment_lists_as_markdown(client, journal_copy):
    day = "20990114"
    stream = _apple_health_card_stream()
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "day_summary_transcript.md").write_text(
        "# Apple Health Summary\n\nGlucose readings: 12\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segments/{day}")

    assert response.status_code == 200
    segments = response.get_json()["segments"]
    health_segment = next(seg for seg in segments if seg["stream"] == stream)
    assert health_segment["types"] == ["markdown"]
    assert health_segment["data_state"] == {"markdown": "analyzed"}
    assert health_segment["think"] is None

    day_response = client.get(f"/app/transcripts/api/day/{day}")
    assert day_response.status_code == 200
    day_data = day_response.get_json()
    assert day_data["audio"] == []
    assert day_data["screen"] == []
    assert day_data["segments"] == [health_segment]


def test_markdown_only_import_segment_renders_markdown_chunk(client, journal_copy):
    day = "20990114"
    stream = _apple_health_card_stream()
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "day_summary_transcript.md").write_text(
        "# Apple Health Summary\n\nGlucose readings: 12\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["audio_file"] is None
    assert data["duration"] == 0.0
    assert data["media_sizes"] == {"audio": 0, "screen": 0}
    assert data["media_purged"] == {"audio": False, "screen": False}
    assert data["data_state"] == {"markdown": "analyzed"}
    assert len(data["chunks"]) == 1
    chunk = data["chunks"][0]
    assert chunk["type"] == "markdown"
    assert chunk["time"] == "00:00:00"
    assert chunk["timestamp"] == _timestamp_from_day_time(day, "00:00:00")
    assert chunk["markdown"] == "# Apple Health Summary\n\nGlucose readings: 12"
    assert chunk["source_ref"] == {"filename": "day_summary_transcript.md"}


def test_image_import_segment_renders_markdown_and_ignores_preserved_original(
    client, journal_copy
):
    day = "20990216"
    stream = "import.image"
    segment = "000000_300"
    transcript = "# Imported Image\n\nExtracted text from the preserved scan."
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "original.jpg").write_bytes(b"preserved-jpeg")
    (segment_dir / "image_transcript.md").write_text(
        transcript + "\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    markdown_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "markdown"]
    assert len(markdown_chunks) == 1
    assert markdown_chunks[0]["markdown"] == transcript
    assert data["data_state"] == {"markdown": "analyzed"}
    assert data["media_sizes"] == {"audio": 0, "screen": 0}
    assert data["media_purged"] == {"audio": False, "screen": False}


def test_document_import_segment_renders_markdown_transcript(client, journal_copy):
    day = "20990217"
    stream = "import.document"
    segment = "000000_300"
    transcript = "# Imported Document\n\nContract section two text."
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "original.pdf").write_bytes(b"%PDF-1.4 synthetic")
    (segment_dir / "document_transcript.md").write_text(
        transcript + "\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    markdown_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "markdown"]
    assert len(markdown_chunks) == 1
    assert markdown_chunks[0]["markdown"] == transcript
    assert data["data_state"] == {"markdown": "analyzed"}


def test_image_import_segment_lists_as_markdown(client, journal_copy):
    day = "20990218"
    stream = "import.image"
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "original.jpg").write_bytes(b"list-jpeg")
    (segment_dir / "image_transcript.md").write_text(
        "# Image List\n\nGallery import transcript.\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segments/{day}")

    assert response.status_code == 200
    segments = response.get_json()["segments"]
    image_segment = next(seg for seg in segments if seg["stream"] == stream)
    assert image_segment["types"] == ["markdown"]
    assert image_segment["data_state"] == {"markdown": "analyzed"}


def test_image_import_segment_with_empty_transcript_reports_no_screen_state(
    client, journal_copy
):
    day = "20990219"
    stream = "import.image"
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "original.jpg").write_bytes(b"empty-transcript-jpeg")
    (segment_dir / "image_transcript.md").write_text(
        "   \n\n",
        encoding="utf-8",
    )

    list_response = client.get(f"/app/transcripts/api/segments/{day}")
    assert list_response.status_code == 200
    segments = list_response.get_json()["segments"]
    image_segment = next(seg for seg in segments if seg["stream"] == stream)
    assert image_segment["types"] == ["markdown"]
    assert image_segment["data_state"] == {"markdown": "analyzed"}

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert [chunk for chunk in data["chunks"] if chunk["type"] == "markdown"] == []
    assert "screen" not in data["data_state"]
    assert "audio" not in data["data_state"]
    assert data["media_sizes"]["screen"] == 0


def test_import_segment_with_screen_jsonl_is_not_markdown_only(client, journal_copy):
    day = "20990220"
    stream = "import.image"
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "original.jpg").write_bytes(b"screen-jsonl-jpeg")
    (segment_dir / "image_transcript.md").write_text(
        "# Screen Import\n\nThis transcript must not make the segment markdown-only.\n",
        encoding="utf-8",
    )
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {
                "frame_id": 1,
                "timestamp": 0,
                "analysis": {"primary": "media"},
                "content": {},
            }
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"] == {"screen": "analyzed"}
    assert any(chunk["type"] == "screen" for chunk in data["chunks"])
    assert data["media_sizes"]["screen"] == len(b"screen-jsonl-jpeg")


def test_non_import_stream_with_transcript_md_is_not_markdown_only(
    client, journal_copy
):
    day = "20990221"
    stream = "default"
    segment = "000000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "stream.json").write_text(
        json.dumps({"stream": stream}),
        encoding="utf-8",
    )
    (segment_dir / "note_transcript.md").write_text(
        "# Local Transcript\n\nDefault stream transcript text.\n",
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segments/{day}")

    assert response.status_code == 200
    segments = response.get_json()["segments"]
    default_segment = next(seg for seg in segments if seg["stream"] == stream)
    assert default_segment["types"] == ["audio"]
    assert default_segment["data_state"] == {"audio": "analyzed"}


def test_segment_content_marks_headerless_screen_frame_analyzed(client, journal_copy):
    day = "20990115"
    stream = "default"
    segment = "090000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    frame = {
        "frame_id": 1,
        "timestamp": 1,
        "analysis": {
            "primary": "work",
            "visual_description": "fedora tmux session",
        },
        "content": {},
    }
    _write_jsonl(segment_dir / "fedora_tmux_screen.jsonl", [frame])

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"] == {"screen": "analyzed"}
    assert any(chunk["type"] == "screen" for chunk in data["chunks"])


def test_segment_content_renders_sense_json_over_stale_markdown(client, journal_copy):
    day = "20990116"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, audio=False, screen=False)
    talents_dir = journal_copy / "chronicle" / day / stream / segment / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    (talents_dir / "sense.json").write_text(
        json.dumps(
            {
                "density": "active",
                "content_type": "meeting",
                "activity_summary": "Discussed the timeline for the launch.",
                "entities": [
                    {
                        "type": "Person",
                        "name": "Alice Smith",
                        "role": "attendee",
                        "source": "voice",
                        "context": "Owned timeline follow-up.",
                    },
                    {
                        "type": "Tool",
                        "name": "Grafana",
                        "role": "mentioned",
                        "source": "screen",
                        "context": "Used for dashboards.",
                    },
                ],
                "facets": [
                    {"facet": "work", "activity": "launch planning", "level": "high"}
                ],
                "speculative_facet": None,
                "meeting_detected": True,
                "speakers": ["Alice Smith", "Bob Chen"],
                "recommend": {"screen_record": True, "speaker_attribution": True},
                "emotional_register": "collaborative",
            }
        ),
        encoding="utf-8",
    )
    (talents_dir / "sense.md").write_text("STALE MD", encoding="utf-8")

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    md_files = response.get_json()["md_files"]
    assert "entities" not in md_files
    assert list(md_files).count("sense") == 1
    assert "Alice Smith" in md_files["sense"]
    assert "Owned timeline follow-up." in md_files["sense"]
    assert "Discussed the timeline for the launch." in md_files["sense"]
    assert "STALE MD" not in md_files["sense"]


def test_segment_content_renders_documents_json_projection(client, journal_copy):
    day = "20990118"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, audio=False, screen=False)
    talents_dir = journal_copy / "chronicle" / day / stream / segment / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    (talents_dir / "documents.json").write_text(
        json.dumps(
            {
                "overview": "Miller Family Trust Amendment executed March 4, 2026.",
                "parties": [
                    {
                        "name": "Priya Shah",
                        "role": "primary trustee",
                        "formal_term": "Trustee",
                        "appointment_tier": "primary",
                        "context": "Appointed to administer the trust.",
                    }
                ],
                "key_provisions": [],
                "assets": [],
                "conditions": [],
                "important_dates": [
                    {
                        "date": "March 4, 2026",
                        "meaning": "Execution date.",
                    }
                ],
                "summary": "Priya Shah is appointed as primary trustee.",
            }
        ),
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    md_files = response.get_json()["md_files"]
    assert "documents" in md_files
    assert "## Overview" in md_files["documents"]
    assert "## Parties and Roles" in md_files["documents"]
    assert "Priya Shah - primary trustee (Trustee) [primary]" in md_files["documents"]
    assert "## Important Dates" in md_files["documents"]


def test_segment_content_drops_screen_json_projection_when_screen_analyzed(
    client, journal_copy
):
    day = "20990119"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, audio=False, screen=True)
    talents_dir = journal_copy / "chronicle" / day / stream / segment / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)
    (talents_dir / "screen.json").write_text(
        json.dumps(
            {
                "narrative": "09:00 Alice Smith reviewed the launch board.",
                "entities": [
                    {
                        "type": "Person",
                        "name": "Alice Smith",
                        "role": "attendee",
                        "context": "Visible in the meeting participant tile.",
                    }
                ],
            }
        ),
        encoding="utf-8",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"].get("screen") == "analyzed"
    assert "screen" not in data["md_files"]


def test_segment_content_maps_still_images_to_screen_frames(client, journal_copy):
    day = "20990117"
    stream = "mentra-live"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "mentra-photo-1.jpg").write_bytes(b"first-image")
    (segment_dir / "mentra-photo-2.jpg").write_bytes(b"second-image")
    _write_jsonl(
        segment_dir / "mentra-photo-1.jsonl",
        [
            {"raw": "mentra-photo-1.jpg", "kind": "image"},
            {"start": "00:00:00", "text": "First Mentra image description."},
        ],
    )
    _write_jsonl(
        segment_dir / "mentra-photo-2.jsonl",
        [
            {"raw": "mentra-photo-2.jpg", "kind": "image"},
            {"start": "00:00:00", "text": "Second Mentra image description."},
        ],
    )
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "mentra-photo-1.jpg", "modality": "photo"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "analysis": {
                    "primary": "media",
                    "visual_description": "Mentra Live photo captured.",
                },
                "content": {"media": {"description": ""}},
            },
            {
                "frame_id": 1,
                "timestamp": 10,
                "analysis": {
                    "primary": "media",
                    "visual_description": "Mentra Live photo captured.",
                },
                "content": {"media": {"description": ""}},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "screen"]
    assert data["image_files"] == {
        "mentra-photo-1.jpg": (
            f"/app/transcripts/api/serve_file/{day}/{stream}/{segment}/mentra-photo-1.jpg"
        ),
        "mentra-photo-2.jpg": (
            f"/app/transcripts/api/serve_file/{day}/{stream}/{segment}/mentra-photo-2.jpg"
        ),
    }
    assert [chunk["source_ref"]["raw"] for chunk in screen_chunks] == [
        "mentra-photo-1.jpg",
        "mentra-photo-2.jpg",
    ]
    assert [chunk["source_ref"]["media_kind"] for chunk in screen_chunks] == [
        "image",
        "image",
    ]
    assert "First Mentra image description." in screen_chunks[0]["markdown"]
    assert "Second Mentra image description." in screen_chunks[1]["markdown"]
    assert data["media_sizes"]["screen"] == len(b"first-image") + len(b"second-image")


def test_segment_content_renders_media_category_string_frame(client, journal_copy):
    day = "20990119"
    stream = "default"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "frame-1.jpg").write_bytes(b"image-bytes")
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "frame-1.jpg", "modality": "screen"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "raw": "frame-1.jpg",
                "analysis": {"primary": "media"},
                "content": {"media": "A video player showing a nature documentary.\n"},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_warnings = [
        detail
        for detail in data.get("warning_details", [])
        if detail["type"] == "screen"
    ]
    assert screen_warnings == []
    screen_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "screen"]
    assert screen_chunks
    assert (
        "A video player showing a nature documentary." in screen_chunks[0]["markdown"]
    )


def test_segment_content_renders_meeting_string_frame(client, journal_copy):
    day = "20990120"
    stream = "default"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "screen.webm", "modality": "screen"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "analysis": {"primary": "meeting"},
                "content": {"meeting": "Two people on a video call.\n"},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_warnings = [
        detail
        for detail in data.get("warning_details", [])
        if detail["type"] == "screen"
    ]
    assert screen_warnings == []
    screen_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "screen"]
    assert screen_chunks
    assert screen_chunks[0]["source_ref"]["participants"] is None


def test_segment_content_extracts_meeting_participant_boxes(client, journal_copy):
    day = "20990121"
    stream = "default"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "screen.webm", "modality": "screen"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "analysis": {"primary": "meeting"},
                "content": {
                    "meeting": {
                        "platform": "zoom",
                        "participants": [
                            {
                                "name": "Alice",
                                "status": "speaking",
                                "video": True,
                                "box_2d": [100, 200, 500, 600],
                            }
                        ],
                    }
                },
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_warnings = [
        detail
        for detail in data.get("warning_details", [])
        if detail["type"] == "screen"
    ]
    assert screen_warnings == []
    screen_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "screen"]
    assert screen_chunks
    assert screen_chunks[0]["source_ref"]["participants"] == [
        {
            "name": "Alice",
            "status": "speaking",
            "top": 10.0,
            "left": 20.0,
            "height": 40.0,
            "width": 40.0,
        }
    ]


def test_segment_content_renders_image_frame_with_nondict_analysis(
    client, journal_copy
):
    day = "20990122"
    stream = "default"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "frame-1.jpg").write_bytes(b"image-bytes")
    _write_jsonl(
        segment_dir / "frame-1.jsonl",
        [{"text": "A reference image description."}],
    )
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "frame-1.jpg", "modality": "screen"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "raw": "frame-1.jpg",
                "analysis": "work",
                "content": {"media": {"description": ""}},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_warnings = [
        detail
        for detail in data.get("warning_details", [])
        if detail["type"] == "screen"
    ]
    assert screen_warnings == []
    screen_chunks = [chunk for chunk in data["chunks"] if chunk["type"] == "screen"]
    assert screen_chunks


def test_segment_content_returns_deduped_signal_context(client, journal_copy):
    day = "20990118"
    stream = "mentra-live"
    segment = "084300_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "audio.wav", "duration": 60},
            {"start": "08:43:05", "source": "mentra", "speaker": 1, "text": "hello"},
        ],
    )
    _write_jsonl(
        segment_dir / "signals.jsonl",
        [
            {
                "timestamp": "2099-01-18T15:43:10Z",
                "event_type": "location_update",
                "payload": {
                    "lat": 39.7,
                    "lng": -104.9,
                    "accuracy": 13,
                    "timestamp": "2099-01-18T15:43:10Z",
                },
            },
            {
                "timestamp": "2099-01-18T15:43:12Z",
                "event_type": "calendar_event",
                "payload": {
                    "eventId": "evt-1",
                    "title": "Team Daily Standup",
                    "dtStart": "2099-01-18T16:15:00.000Z",
                    "dtEnd": "2099-01-18T16:30:00.000Z",
                    "timezone": "MST",
                },
            },
            {
                "timestamp": "2099-01-18T15:43:30Z",
                "event_type": "calendar_event",
                "payload": {
                    "eventId": "evt-1",
                    "title": "Team Daily Standup",
                    "dtStart": "2099-01-18T16:15:00.000Z",
                    "dtEnd": "2099-01-18T16:30:00.000Z",
                    "timezone": "MST",
                },
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["signals"]["counts"] == {
        "calendar_event": 2,
        "location_update": 1,
    }
    assert data["signals"]["calendar"]["total"] == 2
    assert data["signals"]["calendar"]["unique"] == 1
    assert data["signals"]["calendar"]["events"][0]["seen_count"] == 2
    assert data["signals"]["calendar"]["events"][0]["title"] == "Team Daily Standup"
    assert data["signals"]["events"][0]["event_type"] == "location_update"
    assert data["signals"]["events"][0]["time"]


def test_segment_content_strips_duplicate_audio_markdown_timestamp(
    client, journal_copy
):
    day = "20990110"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "raw.m4a", "duration": 42.0},
            {
                "start": "00:00:05",
                "source": "mic",
                "speaker": 1,
                "text": "hello from the room",
            },
            {
                "start": "00:00:10",
                "source": "sys",
                "speaker": 2,
                "text": "system audio line",
            },
        ],
    )
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "screen.webm"},
            {
                "frame_id": 1,
                "timestamp": 7,
                "analysis": {
                    "primary": "work",
                    "visual_description": "[09:00:07] screen bracket stays",
                },
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    chunks = response.get_json()["chunks"]
    audio_chunks = [chunk for chunk in chunks if chunk["type"] == "audio"]
    assert [chunk["time"] for chunk in audio_chunks] == ["00:00:05", "00:00:10"]
    assert all(
        not re.match(r"^\[\d{2}:\d{2}:\d{2}\]", chunk["markdown"])
        for chunk in audio_chunks
    )
    assert audio_chunks[0]["markdown"].startswith("(mic) hello")
    assert "Speaker 1:" not in audio_chunks[0]["markdown"]

    screen_chunk = next(chunk for chunk in chunks if chunk["type"] == "screen")
    assert "[09:00:07] screen bracket stays" in screen_chunk["markdown"]


def test_segment_content_returns_warning_details_for_parse_failures(
    client, journal_copy
):
    day = "20990106"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    (segment_dir / "audio.jsonl").write_text("{bad json\n", encoding="utf-8")
    (segment_dir / "screen.jsonl").write_text("{bad json\n", encoding="utf-8")

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["warnings"] == 2
    assert [detail["type"] for detail in data["warning_details"]] == [
        "audio",
        "screen",
    ]
    assert all(detail["file"] for detail in data["warning_details"])
    assert all(detail["message"] for detail in data["warning_details"])
    assert all(detail["ts"] for detail in data["warning_details"])
    assert data["data_state"] == {"audio": "failed", "screen": "failed"}
    assert data["media_purged"] == {"audio": False, "screen": False}


def test_segment_content_screen_media_description_string(client, journal_copy):
    day = "20990106"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "screen.webm"},
            {
                "frame_id": 1,
                "timestamp": 1,
                "analysis": {
                    "primary": "media",
                    "visual_description": "Watching a livestream.",
                },
                "content": {"media": "# [OpenAI - Introducing GPT-Live]"},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["warnings"] == 0
    screen_chunk = next(chunk for chunk in data["chunks"] if chunk["type"] == "screen")
    assert "# [OpenAI - Introducing GPT-Live]" in screen_chunk["markdown"]


def test_segment_content_preserves_image_backed_media_description_string(
    client, journal_copy
):
    day = "20990119"
    stream = "mentra-live"
    segment = "164900_60"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "frame.jpg").write_bytes(b"image-frame")
    _write_jsonl(
        segment_dir / "frame.jsonl",
        [
            {"raw": "frame.jpg", "kind": "image"},
            {"start": "00:00:00", "text": "Image sidecar description."},
        ],
    )
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {"raw": "frame.jpg", "modality": "photo"},
            {
                "frame_id": 1,
                "timestamp": 0,
                "analysis": {
                    "primary": "media",
                    "visual_description": "Mentra Live photo captured.",
                },
                "content": {"media": "# [Original Markdown Media]"},
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    screen_chunk = next(chunk for chunk in data["chunks"] if chunk["type"] == "screen")
    assert "# [Original Markdown Media]" in screen_chunk["markdown"]
    assert '"photo_file": "frame.jpg"' not in screen_chunk["markdown"]
    assert "Image sidecar description." in screen_chunk["markdown"]
    assert screen_chunk["source_ref"]["raw"] == "frame.jpg"
    assert screen_chunk["source_ref"]["media_kind"] == "image"


@pytest.mark.parametrize("raw_name", ["audio.flac", "audio.m4a"])
def test_segment_content_raw_audio_without_jsonl_is_pending(
    client, journal_copy, raw_name
):
    day = "20990111"
    stream = "default"
    segment = "090000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / raw_name).write_bytes(b"audio")

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["chunks"] == []
    assert data["data_state"] == {"audio": "pending"}
    assert data["media_sizes"]["audio"] == 5
    assert data["media_purged"] == {"audio": False, "screen": False}


def test_segment_content_header_only_missing_raw_is_purged(client, journal_copy):
    day = "20990112"
    stream = "default"
    segment = "090000_300"
    _write_segment(
        journal_copy,
        day,
        stream,
        segment,
        audio_state="pending",
        screen_state="pending",
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["chunks"] == []
    assert data["data_state"] == {"audio": "purged", "screen": "purged"}
    assert data["media_purged"] == {"audio": True, "screen": True}


def test_segment_content_empty_record_screen_is_empty(client, journal_copy):
    day = "20990121"
    stream = "default"
    segment = "090000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "screen.webm").write_bytes(b"screen")
    _write_jsonl(segment_dir / "screen.jsonl", [_empty_screen_header()])

    signals = _segment_modality_signals(segment_dir, "screen")
    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert signals["state"] == "empty"
    assert response.status_code == 200
    data = response.get_json()
    assert data["chunks"] == []
    assert data["data_state"] == {"screen": "empty"}
    assert data["media_purged"] == {"audio": False, "screen": False}


def test_segment_content_purged_beats_empty_record(client, journal_copy):
    day = "20990122"
    stream = "default"
    segment = "090000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    _write_jsonl(segment_dir / "screen.jsonl", [_empty_screen_header()])

    signals = _segment_modality_signals(segment_dir, "screen")
    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert signals["state"] == "purged"
    assert response.status_code == 200
    data = response.get_json()
    assert data["chunks"] == []
    assert data["data_state"] == {"screen": "purged"}
    assert data["media_purged"] == {"audio": False, "screen": True}


def test_segment_content_analyzed_missing_raw_keeps_purged_flag(client, journal_copy):
    day = "20990113"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, audio=False)

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert any(chunk["type"] == "screen" for chunk in data["chunks"])
    assert data["data_state"] == {"screen": "analyzed"}
    assert data["media_purged"] == {"audio": False, "screen": True}


def test_segment_content_failed_precedence_over_pending_raw(client, journal_copy):
    day = "20990114"
    stream = "default"
    segment = "090000_300"
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True)
    (segment_dir / "audio.flac").write_bytes(b"audio")
    (segment_dir / "audio.jsonl").write_text("{bad json\n", encoding="utf-8")

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["chunks"] == []
    assert data["warning_details"][0]["type"] == "audio"
    assert data["data_state"] == {"audio": "failed"}


def test_segment_content_returns_audio_header_duration(client, journal_copy):
    day = "20990107"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "raw.m4a", "duration": 123.4},
            {
                "start": "00:00:05",
                "source": "mic",
                "speaker": 1,
                "text": "duration from header",
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    duration = response.get_json()["duration"]
    assert duration == 123.4
    assert isinstance(duration, float)
    assert duration > 0


def test_segment_content_falls_back_to_segment_window_duration(client, journal_copy):
    day = "20990108"
    stream = "default"
    segment = "090000_300"
    _write_segment(journal_copy, day, stream, segment, screen=False)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "raw.m4a"},
            {
                "start": "00:00:05",
                "source": "mic",
                "speaker": 1,
                "text": "duration from segment key",
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    duration = response.get_json()["duration"]
    assert duration == 300.0
    assert isinstance(duration, float)
    assert duration > 0


@pytest.mark.integration
@_requires_ffmpeg
def test_moov_at_tail_m4a_fixture_has_tail_moov_and_true_duration(tmp_path):
    media_path = tmp_path / "tail-moov.m4a"

    build_moov_at_tail_m4a(media_path, 3.0)

    assert read_true_duration_seconds(media_path) == pytest.approx(3.0, abs=0.2)
    atom_order = top_level_atom_order(media_path)
    assert atom_order.index("mdat") < atom_order.index("moov")
    head = head_bytes(media_path, 4096)
    assert b"moov" not in head
    assert b"mvhd" not in head


@_requires_ffmpeg
def test_segment_content_returns_finite_duration_for_moov_at_tail_audio(
    client, journal_copy, tmp_path
):
    day = "20990109"
    stream = "default"
    segment = "090000_300"
    _, true_duration = _write_moov_tail_audio_segment(
        journal_copy, tmp_path, day, stream, segment, 3.0
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    duration = response.get_json()["duration"]
    assert isinstance(duration, float)
    assert math.isfinite(duration)
    assert duration == pytest.approx(true_duration, abs=1.0)


@_requires_ffmpeg
def test_segment_content_does_not_probe_served_m4a(
    client, journal_copy, tmp_path, monkeypatch
):
    day = "20990112"
    stream = "default"
    segment = "090000_300"
    raw_path, _ = _write_moov_tail_audio_segment(
        journal_copy, tmp_path, day, stream, segment, 3.0
    )
    raw_path = raw_path.resolve()

    subprocess_calls = []
    av_calls = []
    m4a_content_reads = []
    original_builtin_open = builtins.open
    original_path_open = Path.open

    def resolved_target(target) -> Path | None:
        try:
            return Path(target).resolve()
        except (TypeError, ValueError, OSError):
            return None

    def subprocess_run_spy(*args, **kwargs):
        subprocess_calls.append(args[0] if args else kwargs.get("args"))
        raise AssertionError("segment_content must not invoke subprocess probes")

    def av_open_spy(path, *args, **kwargs):
        av_calls.append(path)
        raise AssertionError("segment_content must not open raw media with av")

    def builtin_open_spy(file, *args, **kwargs):
        mode = args[0] if args else kwargs.get("mode", "r")
        if resolved_target(file) == raw_path:
            m4a_content_reads.append(("builtins.open", mode))
        return original_builtin_open(file, *args, **kwargs)

    def path_open_spy(self, *args, **kwargs):
        mode = args[0] if args else kwargs.get("mode", "r")
        if self.resolve() == raw_path:
            m4a_content_reads.append(("Path.open", mode))
        return original_path_open(self, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", subprocess_run_spy)
    monkeypatch.setattr(av, "open", av_open_spy)
    monkeypatch.setattr(builtins, "open", builtin_open_spy)
    monkeypatch.setattr(Path, "open", path_open_spy)

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    # os.path.isfile/getsize on raw media are metadata operations, not content reads.
    assert subprocess_calls == []
    assert av_calls == []
    assert m4a_content_reads == []


def test_segment_content_drops_talent_md_when_data_state_analyzed(client):
    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert any(c["type"] == "screen" for c in data["chunks"])
    assert data["data_state"].get("audio") == "analyzed"
    assert "screen" not in data["md_files"]
    assert "audio" not in data["md_files"]


def test_reprocess_segment_rejects_invalid_day(client):
    response = client.post(
        "/app/transcripts/api/segment/2026-05-20/default/090000_300/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't use that day.",
        reason_code="invalid_day",
        detail="Invalid day format",
    )


def test_reprocess_segment_rejects_invalid_segment_or_stream(client):
    response = client.post(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/Upper/090000_300/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Invalid stream format",
    )


def test_reprocess_segment_rejects_invalid_segment_key(client):
    response = client.post(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/default/not-a-segment/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Invalid segment key format",
    )


def test_reprocess_segment_rejects_invalid_body_value(client, journal_copy):
    day = "20990120"
    segment = "090000_300"
    _write_raw_pending_segment(journal_copy, day, "alpha", segment)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "notes"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't use one of those values.",
        reason_code="invalid_request_value",
        detail="modality must be audio or screen",
    )


def test_reprocess_missing_segment_does_not_create_phantom_directory(
    client, journal_copy
):
    response = client.post(
        "/app/transcripts/api/segment/29990101/default/090000_300/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 404
    _assert_reason(
        response,
        error="I couldn't use that day.",
        reason_code="invalid_day",
        detail="Day not found",
    )
    assert not (journal_copy / "chronicle" / "29990101").exists()


def test_reprocess_existing_day_missing_segment_does_not_create_segment(
    client, journal_copy
):
    day = "20990129"
    (journal_copy / "chronicle" / day / "default").mkdir(parents=True)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/default/090000_300/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 404
    _assert_reason(
        response,
        error="I couldn't use that segment or stream.",
        reason_code="invalid_segment_or_stream",
        detail="Segment not found",
    )
    assert not (journal_copy / "chronicle" / day / "default" / "090000_300").exists()


def test_reprocess_segment_rejects_analyzed_modality(client, journal_copy):
    day = "20990121"
    segment = "090000_300"
    _write_segment(journal_copy, day, "alpha", segment, audio=False)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't take that action in the current state.",
        reason_code="invalid_operation_for_state",
        detail="Segment modality is already analyzed",
    )


def test_reprocess_segment_rejects_purged_modality(client, journal_copy):
    day = "20990122"
    segment = "090000_300"
    _write_segment(
        journal_copy,
        day,
        "alpha",
        segment,
        audio=False,
        screen_state="pending",
    )

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't run analysis because the raw media is no longer available.",
        reason_code="raw_media_not_available",
        detail="Raw media is no longer available",
    )


def test_reprocess_segment_rejects_missing_raw_modality(client, journal_copy):
    day = "20990123"
    segment = "090000_300"
    (journal_copy / "chronicle" / day / "alpha" / segment).mkdir(parents=True)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 400
    _assert_reason(
        response,
        error="I couldn't run analysis because the raw media is no longer available.",
        reason_code="raw_media_not_available",
        detail="Raw media is no longer available",
    )


def test_reprocess_segment_starts_sense_process(client, journal_copy, monkeypatch):
    day = "20990124"
    segment = "090000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    popen_calls, threads = _stub_reprocess_spawn(monkeypatch)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"]["screen"] == "analyzing"
    assert data["marker"]["started_at"]
    assert data["repair_status"] == "accepted"
    marker = segment_dir / ".analyzing_screen"
    assert marker.exists()
    marker_payload = json.loads(marker.read_text())
    assert marker_payload["modality"] == "screen"
    assert marker_payload["request_id"]
    assert len(popen_calls) == 1
    argv, kwargs = popen_calls[0]
    assert argv == [
        "journal",
        "sense",
        "--day",
        day,
        "--segment",
        segment,
        "--stream",
        "alpha",
        "--reprocess",
        "screen",
    ]
    assert kwargs["start_new_session"] is True
    assert kwargs["stdin"] == subprocess.DEVNULL
    assert kwargs["stdout"] == subprocess.PIPE
    assert kwargs["stderr"] == subprocess.PIPE
    assert len(threads) == 1
    assert threads[0].daemon is True
    assert threads[0].started is True
    assert threads[0].args[1] == marker
    assert threads[0].args[2] == segment_dir / ".analyze_failed_screen"
    assert threads[0].args[3] == segment_dir
    assert threads[0].args[4] == "screen"
    assert threads[0].args[5] == marker_payload["request_id"]


def test_reprocess_segment_analyzing_is_idempotent(client, journal_copy, monkeypatch):
    day = "20990125"
    segment = "090000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    _write_analyzing_marker(segment_dir)

    def fail_popen(*args, **kwargs):
        raise AssertionError("idempotent analyzing request must not spawn")

    monkeypatch.setattr("solstone.apps.transcripts.routes.subprocess.Popen", fail_popen)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"]["screen"] == "analyzing"
    assert data["marker"] == {"started_at": "2026-05-20T09:00:00Z"}
    assert data["repair_status"] == "running"


@pytest.mark.parametrize(
    "marker_age_seconds",
    [ANALYZING_STALE_SECONDS + 60, 3600, 6 * 3600],
)
def test_reprocess_segment_stale_analyzing_marker_respawns(
    client, journal_copy, monkeypatch, marker_age_seconds
):
    day = "20990125"
    segment = "090500_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    marker = _write_analyzing_marker(segment_dir, request_id="aged-req")
    marker_time = time.time() - marker_age_seconds
    os.utime(marker, (marker_time, marker_time))
    popen_calls, _threads = _stub_reprocess_spawn(monkeypatch)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["repair_status"] == "accepted"
    assert len(popen_calls) == 1
    fresh_marker = segment_dir / ".analyzing_screen"
    assert fresh_marker.exists()
    marker_payload = json.loads(fresh_marker.read_text(encoding="utf-8"))
    assert marker_payload["modality"] == "screen"
    assert marker_payload["request_id"] != "aged-req"


def test_reprocess_segment_fresh_analyzing_marker_does_not_respawn(
    client, journal_copy, monkeypatch
):
    day = "20990125"
    segment = "090700_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    marker = _write_analyzing_marker(segment_dir, request_id="fresh-req")
    now = time.time()
    os.utime(marker, (now, now))

    def fail_popen(*args, **kwargs):
        raise AssertionError("fresh analyzing request must not spawn")

    monkeypatch.setattr("solstone.apps.transcripts.routes.subprocess.Popen", fail_popen)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["repair_status"] == "running"
    marker_payload = json.loads(marker.read_text(encoding="utf-8"))
    assert marker_payload["request_id"] == "fresh-req"


def test_reprocess_segment_file_exists_race_returns_running(
    client, journal_copy, monkeypatch
):
    day = "20990125"
    segment = "091000_300"
    _write_raw_pending_segment(journal_copy, day, "alpha", segment)

    def race_create(seg_path: Path, modality: str) -> Path:
        _write_analyzing_marker(seg_path, modality=modality, request_id="race-req")
        raise FileExistsError

    def fail_popen(*args, **kwargs):
        raise AssertionError("race request must not spawn")

    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.create_analyzing_marker",
        race_create,
    )
    monkeypatch.setattr("solstone.apps.transcripts.routes.subprocess.Popen", fail_popen)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["data_state"]["screen"] == "analyzing"
    assert data["marker"] == {"started_at": "2026-05-20T09:00:00Z"}
    assert data["repair_status"] == "running"


def test_reprocess_segment_failed_unlinks_failed_marker(
    client, journal_copy, monkeypatch
):
    day = "20990126"
    segment = "090000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    failed = segment_dir / ".analyze_failed_screen"
    failed.write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen", '
        '"reason": "exit_1", "failed_at": "2026-05-20T09:00:10Z", "detail": "x"}\n',
        encoding="utf-8",
    )
    _stub_reprocess_spawn(monkeypatch)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    assert response.get_json()["repair_status"] == "accepted"
    assert not failed.exists()
    assert (segment_dir / ".analyzing_screen").exists()


def test_reprocess_segment_failed_final_unlinks_failed_marker(
    client, journal_copy, monkeypatch
):
    day = "20990126"
    segment = "091000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    _write_jsonl(
        segment_dir / "screen.jsonl",
        [
            {
                "raw": "screen.webm",
                "_solstone_processing": {
                    "state": STATE_FAILED,
                    "reason_code": "analysis_failed",
                    "attempts": FAILED_ATTEMPT_BOUND,
                },
            }
        ],
    )
    failed = segment_dir / ".analyze_failed_screen"
    failed.write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen", '
        '"reason": "exit_1", "failed_at": "2026-05-20T09:00:10Z", "detail": "x"}\n',
        encoding="utf-8",
    )
    _stub_reprocess_spawn(monkeypatch)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    assert response.get_json()["repair_status"] == "accepted"
    assert not failed.exists()
    assert (segment_dir / ".analyzing_screen").exists()


def test_reprocess_segment_rolls_back_marker_when_spawn_fails(
    client, journal_copy, monkeypatch
):
    day = "20990127"
    segment = "090000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)

    def fail_popen(*args, **kwargs):
        raise OSError("no process")

    monkeypatch.setattr("solstone.apps.transcripts.routes.subprocess.Popen", fail_popen)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 500
    _assert_reason(
        response,
        error="I couldn't read that file.",
        reason_code="file_read_failed",
        detail="Failed to start analysis: no process",
    )
    assert not (segment_dir / ".analyzing_screen").exists()


def test_reprocess_watcher_success_removes_marker(tmp_path):
    (tmp_path / "screen.webm").write_bytes(b"screen")
    _write_jsonl(
        tmp_path / "screen.jsonl",
        [{"raw": "screen.webm"}, {"frame_id": 1, "timestamp": 1, "analysis": {}}],
    )
    marker = _write_analyzing_marker(tmp_path)
    failed = tmp_path / ".analyze_failed_screen"

    _watch_reprocess_completion(
        _ProcStub(rc=0), marker, failed, tmp_path, "screen", "req-1"
    )

    assert not marker.exists()
    assert not failed.exists()


def test_reprocess_watcher_success_empty_removes_marker_without_failed(tmp_path):
    (tmp_path / "screen.webm").write_bytes(b"screen")
    _write_jsonl(tmp_path / "screen.jsonl", [_empty_screen_header()])
    marker = _write_analyzing_marker(tmp_path)
    failed = tmp_path / ".analyze_failed_screen"

    _watch_reprocess_completion(
        _ProcStub(rc=0), marker, failed, tmp_path, "screen", "req-1"
    )

    assert not marker.exists()
    assert not failed.exists()


def test_reprocess_watcher_success_without_chunks_writes_no_output(tmp_path):
    (tmp_path / "screen.webm").write_bytes(b"screen")
    _write_jsonl(tmp_path / "screen.jsonl", [{"raw": "screen.webm"}])
    marker = _write_analyzing_marker(tmp_path)
    failed = tmp_path / ".analyze_failed_screen"

    _watch_reprocess_completion(
        _ProcStub(rc=0), marker, failed, tmp_path, "screen", "req-1"
    )

    assert not marker.exists()
    payload = json.loads(failed.read_text())
    assert payload["reason"] == "no_output"
    assert payload["reason_code"] == "no_output"
    assert payload["detail"] == "worker exited 0 without analyzed chunks"


def test_reprocess_watcher_request_id_mismatch_noops(tmp_path):
    marker = _write_analyzing_marker(tmp_path, request_id="new-req")
    failed = tmp_path / ".analyze_failed_screen"

    _watch_reprocess_completion(
        _ProcStub(rc=7, stderr=b"boom"), marker, failed, tmp_path, "screen", "old-req"
    )

    assert marker.exists()
    assert not failed.exists()


def test_reprocess_watcher_failure_writes_failed_marker(tmp_path):
    marker = _write_analyzing_marker(tmp_path)
    failed = tmp_path / ".analyze_failed_screen"
    stderr = b"x" * 600

    _watch_reprocess_completion(
        _ProcStub(rc=7, stderr=stderr), marker, failed, tmp_path, "screen", "req-1"
    )

    assert not marker.exists()
    payload = json.loads(failed.read_text())
    assert payload["started_at"] == "2026-05-20T09:00:00Z"
    assert payload["modality"] == "screen"
    assert payload["reason"] == "exit_7"
    assert payload["detail"] == "x" * 512
    assert payload["failed_at"]
    assert "reason_code" not in payload


def test_reprocess_segment_isolates_streams(client, journal_copy, monkeypatch):
    day = "20990128"
    segment = "090000_300"
    alpha_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    bravo_dir = _write_raw_pending_segment(journal_copy, day, "bravo", segment)
    popen_calls, _threads = _stub_reprocess_spawn(monkeypatch)

    response = client.post(
        f"/app/transcripts/api/segment/{day}/alpha/{segment}/reprocess",
        json={"modality": "screen"},
    )

    assert response.status_code == 200
    assert (alpha_dir / ".analyzing_screen").exists()
    assert not (bravo_dir / ".analyzing_screen").exists()
    assert "--stream" in popen_calls[0][0]
    assert popen_calls[0][0][popen_calls[0][0].index("--stream") + 1] == "alpha"


def test_delete_segment_happy_path_empties_the_segment_and_leaves_a_tombstone(
    client, journal_copy, monkeypatch, fake_deferred_deletes, retention_executor_available
):
    """The owner's content is gone; the evidence that it was deleted is not.

    ⚠ This replaced an assertion that the segment DIRECTORY vanished. The retention
    executor empties the segment and leaves a tombstone, which is what lets a later
    pass recognise the same segment restored from a backup and remove it again. The
    owner-visible behaviour is unchanged, and the assertion below on the listing is
    what proves it: a tombstoned segment carries no content, so it stops appearing.
    """
    monkeypatch.setattr(routes, "is_supervisor_up", lambda: True)
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )
    assert any(entry.suffix != ".json" for entry in segment_dir.iterdir()), (
        "the fixture must start with real content, or this asserts nothing"
    )

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    pending_id = response.get_json()["pending"]
    assert response.get_json()["deleted"] == FIXTURE_SEGMENT
    fake_deferred_deletes.fire(pending_id)

    remaining = sorted(entry.name for entry in segment_dir.iterdir())
    assert remaining == ["tombstone.json"], remaining
    tombstone = json.loads((segment_dir / "tombstone.json").read_text())
    assert tombstone["manifest"], "the tombstone names what it removed"
    assert tombstone["sanitization_level"] is None, (
        "the executor declines to claim a sanitization level, which is what makes "
        "the rest of the record credible"
    )

    # ⛔ The owner must not see a segment they deleted.
    listing = client.get(f"/app/transcripts/api/segments/{FIXTURE_DAY}")
    assert listing.status_code == 200
    assert all(
        entry.get("segment") != FIXTURE_SEGMENT
        for entry in listing.get_json()["segments"]
    ), listing.get_json()["segments"]


def test_delete_segment_includes_search_index_warning_when_supervisor_is_down(
    client, fake_deferred_deletes
):
    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["success"] is True
    assert data["deleted"] == FIXTURE_SEGMENT
    assert data["search_index_warning"] is True
    assert fake_deferred_deletes.scheduled[0][0] == data["pending"]


def test_delete_segment_omits_search_index_warning_when_supervisor_is_up(
    client, monkeypatch, fake_deferred_deletes
):
    monkeypatch.setattr(routes, "is_supervisor_up", lambda: True)

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["deleted"] == FIXTURE_SEGMENT
    assert "search_index_warning" not in data
    assert fake_deferred_deletes.scheduled[0][0] == data["pending"]


def test_delete_segment_returns_pending_response_shape(
    client, monkeypatch, fake_deferred_deletes
):
    monkeypatch.setattr(routes, "is_supervisor_up", lambda: True)
    before_ms = int(time.time() * 1000)

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["success"] is True
    assert data["deleted"] == FIXTURE_SEGMENT
    assert re.fullmatch(r"[0-9a-f]{32}", data["pending"])
    assert data["ttl_seconds"] == routes.SEGMENT_DELETE_TTL
    assert data["commit_at_ms"] >= before_ms
    scheduled_id, commit_fn, ttl_seconds = fake_deferred_deletes.scheduled[0]
    assert scheduled_id == data["pending"]
    assert callable(commit_fn)
    assert ttl_seconds == routes.SEGMENT_DELETE_TTL


def test_cancel_delete_segment_within_window_keeps_directory(
    client, journal_copy, fake_deferred_deletes
):
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )

    delete_response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    pending_id = delete_response.get_json()["pending"]

    cancel_response = client.post(f"/app/transcripts/api/cancel-delete/{pending_id}")

    assert cancel_response.status_code == 200
    assert cancel_response.get_json() == {"cancelled": pending_id}
    assert segment_dir.exists()
    assert fake_deferred_deletes.cancel(pending_id) is False


def test_cancel_delete_segment_too_late_after_commit(
    client, journal_copy, fake_deferred_deletes, retention_executor_available
):
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )

    delete_response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    pending_id = delete_response.get_json()["pending"]

    fake_deferred_deletes.fire(pending_id)
    cancel_response = client.post(f"/app/transcripts/api/cancel-delete/{pending_id}")

    assert cancel_response.status_code == 410
    _assert_reason(
        cancel_response,
        error="I couldn't finish because that action is no longer available.",
        reason_code="operation_no_longer_available",
        detail="already committed or unknown",
    )
    assert sorted(entry.name for entry in segment_dir.iterdir()) == ["tombstone.json"]


def test_cancel_delete_segment_unknown_pending_id_returns_410(client):
    response = client.post(f"/app/transcripts/api/cancel-delete/{'a' * 32}")

    assert response.status_code == 410
    _assert_reason(
        response,
        error="I couldn't finish because that action is no longer available.",
        reason_code="operation_no_longer_available",
        detail="already committed or unknown",
    )


def test_cancel_delete_segment_malformed_pending_id_returns_410(client):
    response = client.post("/app/transcripts/api/cancel-delete/not-hex")

    assert response.status_code == 410
    _assert_reason(
        response,
        error="I couldn't finish because that action is no longer available.",
        reason_code="operation_no_longer_available",
        detail="already committed or unknown",
    )


def test_delete_segment_writes_pending_and_committed_audit_rows(
    client, journal_copy, monkeypatch, fake_deferred_deletes, retention_executor_available
):
    monkeypatch.setattr(routes, "is_supervisor_up", lambda: True)

    delete_response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    pending_id = delete_response.get_json()["pending"]

    day_rows = _action_log_rows(journal_copy, FIXTURE_DAY)
    assert any(
        row["action"] == "segment_delete"
        and row["params"].get("pending_id") == pending_id
        and row["params"].get("phase") == "pending"
        for row in day_rows
    )

    fake_deferred_deletes.fire(pending_id)
    day_rows = _action_log_rows(journal_copy, FIXTURE_DAY)
    committed = [
        row
        for row in day_rows
        if row["action"] == "segment_delete"
        and row["params"].get("pending_id") == pending_id
        and row["params"].get("phase") == "committed"
    ]
    assert committed, day_rows
    # ⛔ The audit row carries what the executor actually removed, not just that it
    # was asked to. A row saying "committed" with nothing removed is the shape a
    # silent failure would wear.
    assert committed[0]["params"]["removed"], committed[0]


def test_delete_segment_records_a_failure_when_the_executor_is_unavailable(
    client, journal_copy, monkeypatch, fake_deferred_deletes
):
    """🔴 No executor means no deletion, and the audit row must say so.

    Every removal of the owner's media goes through the retention executor, so if it
    cannot be run nothing is deleted. ⛔ The one outcome that must be impossible is a
    row reading `committed` over content that is still on disk -- an owner believing a
    deletion happened when it did not.
    """
    monkeypatch.setenv("SOLSTONE_RETENTION_BIN", str(journal_copy / "no-such-binary"))
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    pending_id = response.get_json()["pending"]
    fake_deferred_deletes.fire(pending_id)

    assert (segment_dir / "audio.jsonl").exists() or any(
        segment_dir.iterdir()
    ), "nothing was deleted, which is correct"
    rows = [
        row
        for row in _action_log_rows(journal_copy, FIXTURE_DAY)
        if row["action"] == "segment_delete"
        and row["params"].get("pending_id") == pending_id
    ]
    phases = {row["params"].get("phase") for row in rows}
    assert "failed" in phases, rows
    assert "committed" not in phases, (
        "a deletion that did not happen must never be logged as committed"
    )


def test_cancel_delete_segment_writes_cancelled_audit_row(
    client, journal_copy, fake_deferred_deletes
):
    cancel_response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    cancel_pending_id = cancel_response.get_json()["pending"]
    cancel_result = client.post(
        f"/app/transcripts/api/cancel-delete/{cancel_pending_id}"
    )

    assert cancel_result.status_code == 200
    cancel_day = datetime.now().strftime("%Y%m%d")
    cancel_rows = _action_log_rows(journal_copy, cancel_day)
    assert any(
        row["action"] == "segment_delete"
        and row["params"].get("pending_id") == cancel_pending_id
        and row["params"].get("phase") == "cancelled"
        for row in cancel_rows
    )
    assert fake_deferred_deletes.cancel(cancel_pending_id) is False


def test_segment_content_audio_timestamps_are_absolute_not_midnight(
    client, journal_copy
):
    # Audio "start" values are segment-relative offsets; the API timestamp
    # must resolve to the segment's wall-clock time, not just past midnight
    # (upstream PR review finding, 2026-07-04).
    day = "20990110"
    stream = "default"
    segment = "093000_300"
    _write_segment(journal_copy, day, stream, segment)
    segment_dir = journal_copy / "chronicle" / day / stream / segment
    _write_jsonl(
        segment_dir / "audio.jsonl",
        [
            {"raw": "raw.m4a", "duration": 42.0},
            {
                "start": "00:00:05",
                "source": "mic",
                "speaker": 1,
                "text": "daytime words",
            },
        ],
    )

    response = client.get(f"/app/transcripts/api/segment/{day}/{stream}/{segment}")

    assert response.status_code == 200
    chunks = response.get_json()["chunks"]
    audio_chunk = next(chunk for chunk in chunks if chunk["type"] == "audio")
    stamped = datetime.fromtimestamp(audio_chunk["timestamp"] / 1000)
    assert (stamped.hour, stamped.minute, stamped.second) == (9, 30, 5)
