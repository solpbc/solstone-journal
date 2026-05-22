# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import re
import time
from datetime import datetime

import pytest

from solstone.apps.transcripts.routes import _attach_streams_to_ranges

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
) -> None:
    segment_dir = journal_root / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    if audio:
        (segment_dir / "audio.jsonl").write_text("{}\n", encoding="utf-8")
    if screen:
        (segment_dir / "screen.jsonl").write_text(
            '{"raw": "screen.webm"}\n',
            encoding="utf-8",
        )


def _write_jsonl(path, entries: list[dict]) -> None:
    path.write_text(
        "\n".join(json.dumps(entry) for entry in entries) + "\n",
        encoding="utf-8",
    )


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
        {"start": "09:00", "end": "09:15", "streams": ["alpha", "bravo"]}
    ]
    assert data["screen"] == [
        {"start": "09:00", "end": "09:15", "streams": ["alpha", "bravo"]}
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
        }
    ]


def test_ranges_single_stream(client, journal_copy):
    day = "20990104"
    _write_segment(journal_copy, day, "solo", "090000_300", screen=False)

    response = client.get(f"/app/transcripts/api/ranges/{day}")

    assert response.status_code == 200
    assert response.get_json()["audio"] == [
        {"start": "09:00", "end": "09:15", "streams": ["solo"]}
    ]


def test_day_returns_object_shape_with_streams(client, journal_copy):
    day = "20990105"
    _write_segment(journal_copy, day, "alpha", "090000_300")
    _write_segment(journal_copy, day, "bravo", "090500_300", screen=False)

    response = client.get(f"/app/transcripts/api/day/{day}")

    assert response.status_code == 200
    data = response.get_json()
    assert data["audio"] == [
        {"start": "09:00", "end": "09:15", "streams": ["alpha", "bravo"]}
    ]
    assert data["screen"] == [{"start": "09:00", "end": "09:15", "streams": ["alpha"]}]
    assert data["segments"] == [
        {
            "key": "090000_300",
            "start": "09:00",
            "end": "09:05",
            "types": ["audio", "screen"],
            "stream": "alpha",
        },
        {
            "key": "090500_300",
            "start": "09:05",
            "end": "09:10",
            "types": ["audio"],
            "stream": "bravo",
        },
    ]


def test_attach_streams_to_ranges_empty_when_no_overlap():
    result = _attach_streams_to_ranges([("09:00", "09:15")], [], "audio")

    assert result == [{"start": "09:00", "end": "09:15", "streams": []}]


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


def test_segment_content_drops_screen_md_when_screen_chunks_present(client):
    response = client.get(
        f"/app/transcripts/api/segment/{FIXTURE_DAY}/{FIXTURE_STREAM}/{FIXTURE_SEGMENT}"
    )

    assert response.status_code == 200
    data = response.get_json()
    assert any(c["type"] == "screen" for c in data["chunks"])
    assert "screen" not in data["md_files"]
    assert "audio" in data["md_files"]


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
