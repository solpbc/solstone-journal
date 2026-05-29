# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import builtins
import io
import json
import math
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

import av
import pytest

from solstone.apps.transcripts.routes import (
    _attach_streams_to_ranges,
    _watch_reprocess_completion,
)
from solstone.apps.transcripts.tests._media_helpers import (
    build_moov_at_tail_m4a,
    head_bytes,
    read_true_duration_seconds,
    top_level_atom_order,
)

# 20260304 is the canonical fully-analyzed reference day; see
# tests/fixtures/journal/chronicle/20260304/README.md and
# tests/test_reference_day_fixture.py.
FIXTURE_DAY = "20260304"
FIXTURE_STREAM = "default"
FIXTURE_SEGMENT = "090000_300"


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
    assert audio_chunks[0]["markdown"].startswith("(mic) Speaker 1:")

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


def test_moov_at_tail_m4a_fixture_has_tail_moov_and_true_duration(tmp_path):
    media_path = tmp_path / "tail-moov.m4a"

    build_moov_at_tail_m4a(media_path, 3.0)

    assert read_true_duration_seconds(media_path) == pytest.approx(3.0, abs=0.2)
    atom_order = top_level_atom_order(media_path)
    assert atom_order.index("mdat") < atom_order.index("moov")
    head = head_bytes(media_path, 4096)
    assert b"moov" not in head
    assert b"mvhd" not in head


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
    marker = segment_dir / ".analyzing_screen"
    assert marker.exists()
    marker_payload = json.loads(marker.read_text())
    assert marker_payload["modality"] == "screen"
    assert len(popen_calls) == 1
    argv, kwargs = popen_calls[0]
    assert argv == [
        sys.executable,
        "-m",
        "solstone.observe.sense",
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


def test_reprocess_segment_analyzing_is_idempotent(client, journal_copy, monkeypatch):
    day = "20990125"
    segment = "090000_300"
    segment_dir = _write_raw_pending_segment(journal_copy, day, "alpha", segment)
    (segment_dir / ".analyzing_screen").write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen"}\n',
        encoding="utf-8",
    )

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
    marker = tmp_path / ".analyzing_screen"
    failed = tmp_path / ".analyze_failed_screen"
    marker.write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen"}\n',
        encoding="utf-8",
    )

    _watch_reprocess_completion(_ProcStub(rc=0), marker, failed)

    assert not marker.exists()
    assert not failed.exists()


def test_reprocess_watcher_failure_writes_failed_marker(tmp_path):
    marker = tmp_path / ".analyzing_screen"
    failed = tmp_path / ".analyze_failed_screen"
    marker.write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen"}\n',
        encoding="utf-8",
    )
    stderr = b"x" * 600

    _watch_reprocess_completion(_ProcStub(rc=7, stderr=stderr), marker, failed)

    assert not marker.exists()
    payload = json.loads(failed.read_text())
    assert payload["started_at"] == "2026-05-20T09:00:00Z"
    assert payload["modality"] == "screen"
    assert payload["reason"] == "exit_7"
    assert payload["detail"] == "x" * 512
    assert payload["failed_at"]


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


def test_delete_segment_happy_path_removes_segment_directory(
    client, journal_copy, monkeypatch
):
    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.is_supervisor_up", lambda: True
    )
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    assert response.get_json()["deleted"] == FIXTURE_SEGMENT
    time.sleep(0.2)
    assert not segment_dir.exists()


def test_delete_segment_includes_search_index_warning_when_supervisor_is_down(
    client, monkeypatch
):
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)
    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["success"] is True
    assert data["deleted"] == FIXTURE_SEGMENT
    assert data["search_index_warning"] is True
    time.sleep(0.2)


def test_delete_segment_omits_search_index_warning_when_supervisor_is_up(
    client, monkeypatch
):
    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.is_supervisor_up", lambda: True
    )
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    assert response.get_json()["deleted"] == FIXTURE_SEGMENT
    time.sleep(0.2)


def test_delete_segment_returns_pending_response_shape(client, monkeypatch):
    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.is_supervisor_up", lambda: True
    )
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)
    before_ms = int(time.time() * 1000)

    response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert data["success"] is True
    assert data["deleted"] == FIXTURE_SEGMENT
    assert re.fullmatch(r"[0-9a-f]{32}", data["pending"])
    assert data["ttl_seconds"] == 0.05
    assert data["commit_at_ms"] >= before_ms
    time.sleep(0.2)


def test_cancel_delete_segment_within_window_keeps_directory(
    client, journal_copy, monkeypatch
):
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.2)
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
    time.sleep(0.3)
    assert segment_dir.exists()


def test_cancel_delete_segment_too_late_after_commit(client, journal_copy, monkeypatch):
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)
    segment_dir = (
        journal_copy / "chronicle" / FIXTURE_DAY / FIXTURE_STREAM / FIXTURE_SEGMENT
    )

    delete_response = client.delete(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )
    pending_id = delete_response.get_json()["pending"]

    time.sleep(0.2)
    cancel_response = client.post(f"/app/transcripts/api/cancel-delete/{pending_id}")

    assert cancel_response.status_code == 410
    _assert_reason(
        cancel_response,
        error="I couldn't finish because that action is no longer available.",
        reason_code="operation_no_longer_available",
        detail="already committed or unknown",
    )
    assert not segment_dir.exists()


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
    client, journal_copy, monkeypatch
):
    monkeypatch.setattr(
        "solstone.apps.transcripts.routes.is_supervisor_up", lambda: True
    )
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.05)

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

    time.sleep(0.2)
    day_rows = _action_log_rows(journal_copy, FIXTURE_DAY)
    assert any(
        row["action"] == "segment_delete"
        and row["params"].get("pending_id") == pending_id
        and row["params"].get("phase") == "committed"
        for row in day_rows
    )


def test_cancel_delete_segment_writes_cancelled_audit_row(
    client, journal_copy, monkeypatch
):
    monkeypatch.setattr("solstone.apps.transcripts.routes.SEGMENT_DELETE_TTL", 0.2)
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
