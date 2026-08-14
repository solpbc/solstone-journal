#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Deterministic populated journal fixture for the Convey records corpus.

The builders create independent disposable roots under ``/var/tmp``.  They do
not read the clock, modify ``SOLSTONE_JOURNAL``, or invoke an indexer: search
results are supplied separately by the corpus generator's native-call fake.
"""

from __future__ import annotations

import json
import os
import re
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from solstone.observe.processing_record import (
    HANDLER_DESCRIBE,
    HANDLER_TRANSCRIBE,
    REASON_ANALYSIS_FAILED,
    REASON_OK,
    STATE_ANALYZED,
    STATE_FAILED,
    build_processing_record,
)
from solstone.think.journal_stats import load_fresh_day_cache
from solstone.think.stats_schema import DAY_FIELDS, SCHEMA_VERSION

D_FULL = "20260731"
D_CACHE = "20260715"
D_NO_CACHE = "20260801"
D_RAW = "20260915"
D_CORRUPT_CHAT = "20260916"

FULL_STREAM = "field"
NOTES_STREAM = "notes"
FULL_SEGMENT = "090000_300"
ANALYZING_SEGMENT = "100000_120"
FAILED_SEGMENT = "110000_120"
PURGED_SEGMENT = "120000_120"
MARKDOWN_SEGMENT = "130000_60"

_TEMP_DIR = "/var/tmp"
_FIXED_CACHE_MTIME = 4_102_444_800
_DAY_RE = re.compile(r"\d{8}\Z")
_SEGMENT_RE = re.compile(r"\d{6}_\d+\Z")


def _new_root(phase: str) -> Path:
    return Path(tempfile.mkdtemp(prefix=f"records-corpus-{phase}-", dir=_TEMP_DIR))


def _write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def _write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, sort_keys=True) + "\n" for row in rows),
        encoding="utf-8",
    )


def _processing_record(*, state: str, handler: str, reason_code: str) -> dict[str, Any]:
    return build_processing_record(
        state=state,
        reason_code=reason_code,
        handler=handler,
        input_size=1,
        attempted_at="2026-07-01T00:00:00Z",
        attempts=1 if state == STATE_FAILED else None,
    )


def _segment(root: Path, day: str, stream: str, key: str) -> Path:
    directory = root / "chronicle" / day / stream / key
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def _write_analyzed_audio(segment: Path, *, raw_name: str | None = None) -> None:
    header: dict[str, Any] = {
        "_solstone_processing": _processing_record(
            state=STATE_ANALYZED,
            handler=HANDLER_TRANSCRIBE,
            reason_code=REASON_OK,
        )
    }
    if raw_name is not None:
        header["raw"] = raw_name
    _write_jsonl(
        segment / "mic_audio.jsonl",
        [header, {"start": "00:00:01", "end": "00:00:03", "text": "Seeded transcript."}],
    )


def _write_analyzed_screen(segment: Path) -> None:
    _write_jsonl(
        segment / "screen.jsonl",
        [
            {
                "raw": "screen.png",
                "_solstone_processing": _processing_record(
                    state=STATE_ANALYZED,
                    handler=HANDLER_DESCRIBE,
                    reason_code=REASON_OK,
                ),
            },
            {
                "timestamp": 1,
                "frame_id": "frame-1",
                "analysis": {
                    "primary": "work",
                    "visual_description": "A deterministic screen percept.",
                },
            },
        ],
    )


def _write_browser_percept(segment: Path) -> None:
    _write_jsonl(
        segment / "browser_example.jsonl",
        [
            {
                "t": "segment_start",
                "ts": 1_785_496_400_000,
                "site": "example.test",
                "title": "Seeded browser page",
                "url": "https://example.test/seeded",
                "adapter": "browser",
            },
            {"t": "delta", "ts": 1_785_496_401_000, "op": "text", "text": "Seeded browser percept."},
        ],
    )


def _write_fresh_stats_cache(day_dir: Path) -> None:
    _write_json(
        day_dir / "stats.json",
        {
            "schema_version": SCHEMA_VERSION,
            "stats": {field: 0 for field in DAY_FIELDS},
        },
    )
    cache = day_dir / "stats.json"
    os.utime(cache, (_FIXED_CACHE_MTIME, _FIXED_CACHE_MTIME))


def _today_timestamp(day: str, seconds_after_midnight: int) -> int:
    midnight = datetime.strptime(day, "%Y%m%d").replace(tzinfo=timezone.utc)
    return int(midnight.timestamp() * 1000) + seconds_after_midnight * 1000


def _write_today_chat(root: Path, day: str) -> None:
    segment = _segment(root, day, "chat", "090000_300")
    base = _today_timestamp(day, 9 * 60 * 60)
    request_id = "seeded-threaded-request"
    unresolved_id = "seeded-unresolved-request"
    _write_jsonl(
        segment / "chat.jsonl",
        [
            {
                "kind": "owner_message",
                "ts": base,
                "text": "Please summarize the seeded record.",
                "app": "chat",
                "path": f"/app/chat/{day}",
                "facet": "work",
            },
            {
                "kind": "sol_chat_request",
                "ts": base + 1_000,
                "request_id": request_id,
                "summary": "Offer a seeded follow-up.",
                "message": "A seeded request is ready.",
                "category": "follow_up",
                "dedupe": "seeded-threaded",
                "dedupe_window": "24h",
                "since_ts": base,
                "trigger_talent": "flow",
            },
            {
                "kind": "sol_message",
                "ts": base + 2_000,
                "use_id": "seeded-sol-use",
                "text": "Here is the seeded response.",
                "notes": "Seeded notes.",
                "requested_target": None,
                "requested_task": None,
            },
            {
                "kind": "sol_chat_request_superseded",
                "ts": base + 3_000,
                "request_id": request_id,
                "replaced_by": unresolved_id,
            },
            {
                "kind": "owner_chat_open",
                "ts": base + 4_000,
                "request_id": unresolved_id,
                "surface": "chat",
            },
            {
                "kind": "talent_queued",
                "ts": base + 5_000,
                "use_id": "seeded-talent-use",
                "name": "flow",
                "task": "Inspect seeded records",
                "queued_at": base + 5_000,
                "chat_use_id": "seeded-sol-use",
                "ask": "Inspect seeded records",
                "context": "seed corpus",
                "location": "chat",
            },
            {
                "kind": "talent_spawned",
                "ts": base + 6_000,
                "use_id": "seeded-talent-use",
                "name": "flow",
                "task": "Inspect seeded records",
                "started_at": base + 6_000,
            },
            {
                "kind": "talent_finished",
                "ts": base + 7_000,
                "use_id": "seeded-talent-use",
                "name": "flow",
                "summary": "Seeded inspection completed.",
            },
            {
                "kind": "sol_chat_request",
                "ts": base + 8_000,
                "request_id": unresolved_id,
                "summary": "An unresolved seeded request.",
                "message": "Please open this unresolved request.",
                "category": "follow_up",
                "dedupe": "seeded-unresolved",
                "dedupe_window": "24h",
                "since_ts": base + 7_000,
                "trigger_talent": "flow",
            },
        ],
    )


def _write_historical_chat(root: Path) -> None:
    """Write a non-current chat day for the historical-state probe."""
    segment = _segment(root, D_FULL, "chat", "140000_300")
    _write_jsonl(
        segment / "chat.jsonl",
        [
            {
                "kind": "owner_message",
                "ts": 1_785_476_400_000,
                "text": "Historical seeded chat.",
                "app": "chat",
                "path": f"/app/chat/{D_FULL}",
                "facet": "work",
            },
            {
                "kind": "sol_message",
                "ts": 1_785_476_401_000,
                "use_id": "seeded-historical-use",
                "text": "Historical seeded response.",
                "notes": "",
                "requested_target": None,
                "requested_task": None,
            },
        ],
    )


def _require(condition: bool, missing: str) -> None:
    if not condition:
        raise RuntimeError(f"Missing required seeded condition: {missing}")


def _stream_has_file(stream_dir: Path, pattern: str) -> bool:
    return any(
        any(segment.glob(pattern))
        for segment in stream_dir.iterdir()
        if segment.is_dir() and _SEGMENT_RE.fullmatch(segment.name)
    )


def build_unestablished_journal() -> Path:
    """Return a fresh empty root with no configuration directory."""
    return _new_root("unestablished")


def build_established_journal() -> Path:
    """Return a minimal active journal root matching the settings corpus phase."""
    root = _new_root("established")
    _write_json(root / "config" / "journal.json", {"setup": {"completed_at": 1700000000000}})
    return root


def build_corrupt_journal() -> Path:
    """Return a root whose sole journal config is deliberately unparseable."""
    root = _new_root("corrupt")
    path = root / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('{"setup": {"completed_at": 1700000000000}', encoding="utf-8")
    return root


def build_populated_journal(today_day: str) -> tuple[Path, dict[str, int | bool | str]]:
    """Build a complete synthetic records journal and return its root and manifest.

    ``today_day`` is supplied by the capture generator. It cannot overlap a
    fixed scenario day, because the empty/corrupt chat and sparse-day cases must
    remain distinct from the populated current-day chat stream.
    """
    if not _DAY_RE.fullmatch(today_day):
        raise ValueError("today_day must be an eight-digit YYYYMMDD string")
    if today_day in {D_FULL, D_CACHE, D_NO_CACHE, D_RAW, D_CORRUPT_CHAT}:
        raise ValueError("today_day must not overlap a fixed records corpus day")

    root = _new_root("populated")
    chronicle = root / "chronicle"
    fixed_days = (D_FULL, D_CACHE, D_NO_CACHE, D_RAW, D_CORRUPT_CHAT)
    fixed_months = {day[:6] for day in fixed_days}
    for day in (*fixed_days, today_day):
        (chronicle / day).mkdir(parents=True, exist_ok=True)

    journal_config = {
        "setup": {"completed_at": 1700000000000},
        "identity": {"name": "Corpus Owner", "preferred": "Corpus Owner"},
        "agent": {"name": "Corpus Assistant"},
    }
    _write_json(
        root / "config" / "journal.json",
        journal_config,
    )

    full = _segment(root, D_FULL, FULL_STREAM, FULL_SEGMENT)
    _write_analyzed_audio(full, raw_name="mic_audio.flac")
    _write_analyzed_screen(full)
    _write_browser_percept(full)
    (full / "mic_audio.flac").write_bytes(b"fLaC\x00seeded audio")
    (full / "screen.png").write_bytes(b"\x89PNG\r\n\x1a\nseeded image")
    (full / "zero.flac").write_bytes(b"")
    (full / "mic_audio.xyz").write_bytes(b"unregistered seeded media")
    (full / "talents" / "flow.md").parent.mkdir(parents=True, exist_ok=True)
    (full / "talents" / "flow.md").write_text("# Segment flow\n\nSeeded segment output.\n", encoding="utf-8")
    (chronicle / D_FULL / "talents").mkdir(parents=True, exist_ok=True)
    (chronicle / D_FULL / "talents" / "flow.md").write_text("# Daily flow\n\nSeeded daily output.\n", encoding="utf-8")

    analyzing = _segment(root, D_FULL, FULL_STREAM, ANALYZING_SEGMENT)
    (analyzing / "mic_audio.flac").write_bytes(b"fLaC\x00analyzing")
    _write_json(
        analyzing / ".analyzing_audio",
        {
            "started_at": "2026-07-31T10:00:00Z",
            "modality": "audio",
            "request_id": "seeded-analyzing-request",
        },
    )

    failed = _segment(root, D_FULL, FULL_STREAM, FAILED_SEGMENT)
    _write_jsonl(
        failed / "mic_audio.jsonl",
        [
            {
                "raw": "mic_audio.flac",
                "_solstone_processing": _processing_record(
                    state=STATE_FAILED,
                    handler=HANDLER_TRANSCRIBE,
                    reason_code=REASON_ANALYSIS_FAILED,
                ),
            }
        ],
    )
    (failed / "mic_audio.flac").write_bytes(b"fLaC\x00failed")
    _write_json(
        failed / ".analyze_failed_audio",
        {
            "started_at": "2026-07-31T11:00:00Z",
            "modality": "audio",
            "reason": "analysis_failed",
            "failed_at": "2026-07-31T11:00:01Z",
            "detail": "Seeded failed analysis.",
            "reason_code": REASON_ANALYSIS_FAILED,
        },
    )

    purged = _segment(root, D_FULL, FULL_STREAM, PURGED_SEGMENT)
    _write_jsonl(
        purged / "mic_audio.jsonl",
        [
            {
                "raw": "mic_audio.flac",
                "_solstone_processing": _processing_record(
                    state=STATE_ANALYZED,
                    handler=HANDLER_TRANSCRIBE,
                    reason_code=REASON_OK,
                ),
            }
        ],
    )

    markdown_only = _segment(root, D_FULL, NOTES_STREAM, MARKDOWN_SEGMENT)
    (markdown_only / "note.md").write_text("# Seeded note\n\nMarkdown-only segment.\n", encoding="utf-8")

    cache_segment = _segment(root, D_CACHE, FULL_STREAM, "090000_60")
    _write_analyzed_audio(cache_segment)
    _write_fresh_stats_cache(chronicle / D_CACHE)
    no_cache_segment = _segment(root, D_NO_CACHE, FULL_STREAM, "090000_60")
    _write_analyzed_audio(no_cache_segment)

    corrupt_chat = _segment(root, D_CORRUPT_CHAT, "chat", "090000_300")
    (corrupt_chat / "chat.jsonl").write_text("{ this is not valid JSON\n", encoding="utf-8")
    _write_historical_chat(root)
    _write_today_chat(root, today_day)

    _write_json(
        root / "facets" / "work" / "facet.json",
        {
            "title": "Work",
            "description": "Seeded unmuted facet",
            "color": "#336699",
            "emoji": "🧪",
        },
    )
    _write_json(
        root / "facets" / "muted" / "facet.json",
        {
            "title": "Muted",
            "description": "Seeded muted facet",
            "color": "#777777",
            "emoji": "🔇",
            "muted": True,
        },
    )

    _write_jsonl(
        root / "tokens" / f"{D_FULL}.jsonl",
        [
            {
                "timestamp": 1785498000.0,
                "model": "gpt-5-mini",
                "context": "seed.records",
                "segment": FULL_SEGMENT,
                "usage": {"input_tokens": 1000, "output_tokens": 250, "total_tokens": 1250},
            }
        ],
    )

    cache_fresh = load_fresh_day_cache(chronicle / D_CACHE) is not None
    no_cache_absent = load_fresh_day_cache(chronicle / D_NO_CACHE) is None
    today_chat_path = chronicle / today_day / "chat" / "090000_300" / "chat.jsonl"
    today_events = [json.loads(line) for line in today_chat_path.read_text(encoding="utf-8").splitlines()]
    unresolved_index = next(
        (
            index
            for index, event in enumerate(today_events)
            if event.get("kind") == "sol_chat_request"
            and event.get("request_id") == "seeded-unresolved-request"
        ),
        None,
    )
    token_rows = [
        json.loads(line)
        for line in (root / "tokens" / f"{D_FULL}.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    facet_paths = sorted((root / "facets").glob("*/facet.json"))
    stored_config = json.loads((root / "config" / "journal.json").read_text(encoding="utf-8"))

    day_dirs = sorted(path for path in chronicle.iterdir() if path.is_dir())
    stream_dirs = [
        stream_dir
        for day_dir in day_dirs
        for stream_dir in day_dir.iterdir()
        if stream_dir.is_dir()
        and any(
            child.is_dir() and _SEGMENT_RE.fullmatch(child.name)
            for child in stream_dir.iterdir()
        )
    ]
    non_chat_stream_names = {stream_dir.name for stream_dir in stream_dirs if stream_dir.name != "chat"}
    transcript_stream_names = {
        stream_dir.name for stream_dir in stream_dirs if _stream_has_file(stream_dir, "*audio.jsonl")
    }
    screen_stream_names = {
        stream_dir.name for stream_dir in stream_dirs if _stream_has_file(stream_dir, "*screen.jsonl")
    }
    browser_stream_names = {
        stream_dir.name for stream_dir in stream_dirs if _stream_has_file(stream_dir, "browser_*.jsonl")
    }

    manifest: dict[str, int | bool | str] = {
        "day_count": len(day_dirs),
        # The caller-supplied current chat day is deliberately outside the
        # stable corpus topology, so report only the fixed seeded months.
        "month_count": len(fixed_months),
        "spans_multiple_months": len(fixed_months) > 1,
        "today_day": today_day,
        "today_day_seeded": today_chat_path.is_file(),
        "stream_count": len(non_chat_stream_names),
        "transcript_stream_count": len(transcript_stream_names),
        "screen_stream_count": len(screen_stream_names),
        "browser_stream_count": len(browser_stream_names),
        "markdown_only_segment_count": int((markdown_only / "note.md").is_file()),
        "api_read_transcripts_source_present": (full / "mic_audio.jsonl").is_file(),
        "api_read_percepts_source_present": (full / "screen.jsonl").is_file() and (full / "browser_example.jsonl").is_file(),
        "api_read_agents_source_present": (full / "talents" / "flow.md").is_file(),
        "api_read_source_family_count": sum(
            (
                (full / "mic_audio.jsonl").is_file(),
                (full / "screen.jsonl").is_file() and (full / "browser_example.jsonl").is_file(),
                (full / "talents" / "flow.md").is_file(),
            )
        ),
        "processing_state_analyzed_present": (full / "mic_audio.jsonl").is_file(),
        "processing_state_analyzing_present": (analyzing / ".analyzing_audio").is_file(),
        "processing_state_failed_present": (failed / "mic_audio.jsonl").is_file()
        and (failed / ".analyze_failed_audio").is_file(),
        "processing_state_purged_present": (purged / "mic_audio.jsonl").is_file() and not (purged / "mic_audio.flac").exists(),
        "processing_state_count": sum(
            (
                (full / "mic_audio.jsonl").is_file(),
                (analyzing / ".analyzing_audio").is_file(),
                (failed / "mic_audio.jsonl").is_file()
                and (failed / ".analyze_failed_audio").is_file(),
                (purged / "mic_audio.jsonl").is_file() and not (purged / "mic_audio.flac").exists(),
            )
        ),
        "stats_cache_present_day_count": sum((day / "stats.json").is_file() for day in day_dirs),
        "stats_cache_absent_day_count": sum(not (day / "stats.json").is_file() for day in day_dirs),
        "stats_cache_fresh_asserted": cache_fresh,
        "stats_cache_absent_returns_none_asserted": no_cache_absent,
        "chat_day_count": sum((day / "chat").is_dir() for day in day_dirs),
        "chat_today_events_present": bool(today_events),
        "chat_today_threaded_request_present": any(
            event.get("kind") == "sol_chat_request" and event.get("request_id") == "seeded-threaded-request"
            for event in today_events
        ),
        "chat_today_unresolved_request_present": any(
            event.get("kind") == "sol_chat_request" and event.get("request_id") == "seeded-unresolved-request"
            for event in today_events
        ) and unresolved_index is not None and not any(
            event.get("kind") == "sol_message" for event in today_events[unresolved_index + 1 :]
        ),
        "chat_today_sol_message_origin_present": any(
            event.get("kind") == "sol_chat_request"
            and next_event.get("kind") == "sol_message"
            for event, next_event in zip(today_events, today_events[1:])
        ),
        "chat_today_owner_open_event_present": any(event.get("kind") == "owner_chat_open" for event in today_events),
        "chat_corrupt_day_present": (corrupt_chat / "chat.jsonl").is_file(),
        "chat_empty_day_present": (chronicle / D_RAW).is_dir() and not (chronicle / D_RAW / "chat").exists(),
        "media_file_count": sum(
            path.is_file()
            for path in root.rglob("*")
            if path.suffix.lower() in {".flac", ".png", ".xyz"}
        ),
        "audio_media_present": (full / "mic_audio.flac").is_file(),
        "image_media_present": (full / "screen.png").is_file(),
        "zero_byte_media_present": (full / "zero.flac").is_file() and (full / "zero.flac").stat().st_size == 0,
        "existing_unregistered_media_present": (full / "mic_audio.xyz").is_file() and (full / "mic_audio.xyz").stat().st_size > 0,
        "usage_record_count": len(token_rows),
        "segment_usage_record_present": any(
            row.get("segment") == FULL_SEGMENT
            and int(row.get("usage", {}).get("total_tokens", 0)) > 0
            for row in token_rows
        ),
        "identity_present": isinstance(stored_config.get("identity"), dict),
        "identity_preferred_name_present": bool(stored_config["identity"].get("preferred")),
        "agent_name_present": bool(stored_config.get("agent", {}).get("name")),
        "talent_output_day_level_present": (chronicle / D_FULL / "talents" / "flow.md").is_file(),
        "talent_output_segment_level_present": (full / "talents" / "flow.md").is_file(),
        "zero_talent_output_day_present": not (chronicle / D_RAW / "talents").exists(),
        "facet_count": len(facet_paths),
        "muted_facet_present": any(json.loads(path.read_text(encoding="utf-8")).get("muted") is True for path in facet_paths),
        "unmuted_facet_present": any(not json.loads(path.read_text(encoding="utf-8")).get("muted", False) for path in facet_paths),
    }

    required_truths = (
        "spans_multiple_months",
        "today_day_seeded",
        "api_read_transcripts_source_present",
        "api_read_percepts_source_present",
        "api_read_agents_source_present",
        "processing_state_analyzed_present",
        "processing_state_analyzing_present",
        "processing_state_failed_present",
        "processing_state_purged_present",
        "stats_cache_fresh_asserted",
        "stats_cache_absent_returns_none_asserted",
        "chat_today_events_present",
        "chat_today_threaded_request_present",
        "chat_today_unresolved_request_present",
        "chat_today_sol_message_origin_present",
        "chat_today_owner_open_event_present",
        "chat_corrupt_day_present",
        "chat_empty_day_present",
        "audio_media_present",
        "image_media_present",
        "zero_byte_media_present",
        "existing_unregistered_media_present",
        "segment_usage_record_present",
        "identity_present",
        "identity_preferred_name_present",
        "agent_name_present",
        "talent_output_day_level_present",
        "talent_output_segment_level_present",
        "zero_talent_output_day_present",
        "muted_facet_present",
        "unmuted_facet_present",
    )
    for key in required_truths:
        _require(manifest[key] is True, key)
    for key, minimum in (
        ("day_count", 6),
        # Fixed days guarantee three months; today's month must not affect this.
        ("month_count", 3),
        ("stream_count", 2),
        ("transcript_stream_count", 1),
        ("screen_stream_count", 1),
        ("browser_stream_count", 1),
        ("markdown_only_segment_count", 1),
        ("api_read_source_family_count", 3),
        ("processing_state_count", 4),
        ("stats_cache_present_day_count", 1),
        ("stats_cache_absent_day_count", 1),
        ("chat_day_count", 2),
        ("media_file_count", 4),
        ("usage_record_count", 1),
        ("facet_count", 2),
    ):
        _require(int(manifest[key]) >= minimum, key)

    return root, manifest
