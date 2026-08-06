# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Transcript viewer app - browse and playback daily transcripts."""

from __future__ import annotations

import json
import logging
import os
import re
import subprocess
import sys
import threading
import time
import uuid
from datetime import date, datetime
from glob import glob
from pathlib import Path
from typing import Any

from flask import (
    Blueprint,
    current_app,
    jsonify,
    redirect,
    request,
    send_file,
    url_for,
)

import solstone.think.deferred_deletes as deferred_deletes
from solstone.apps.transcripts.copy import (
    SPEAKER_LABEL_SOURCE_AMBIGUOUS_MESSAGE,
    SPEAKER_LABELS_UNAVAILABLE_MESSAGE,
    transcripts_copy_payload,
)
from solstone.apps.utils import log_app_action
from solstone.convey.date_nav import build_date_nav_index
from solstone.convey.reasons import (
    FILE_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_DAY,
    INVALID_MONTH,
    INVALID_OPERATION_FOR_STATE,
    INVALID_REQUEST_VALUE,
    INVALID_SEGMENT_OR_STREAM,
    OPERATION_NO_LONGER_AVAILABLE,
    RAW_MEDIA_NOT_AVAILABLE,
)
from solstone.convey.utils import (
    DATE_RE,
    error_response,
    safe_day_path,
    success_response,
)
from solstone.observe.hear import format_audio
from solstone.observe.screen import format_screen
from solstone.observe.utils import AUDIO_EXTENSIONS, IMAGE_EXTENSIONS, VIDEO_EXTENSIONS
from solstone.think.browser_formatter import format_browser
from solstone.think.cluster import (
    cluster,
    cluster_period,
    cluster_range,
    cluster_segments,
    cluster_span,
    scan_day,
)
from solstone.think.data_state import (
    DataState,
    create_analyzing_marker,
    derive_modality_state,
    read_processing_record,
    repair_modality_markers,
)
from solstone.think.entities.journal import get_journal_principal, load_journal_entity
from solstone.think.journal_io.npz import load_npz
from solstone.think.journal_stats import load_fresh_day_cache
from solstone.think.media import MIME_TYPES
from solstone.think.models import get_usage_cost
from solstone.think.pipeline_health import (
    lookup_segment_progress,
    read_segment_progress,
    segment_fully_sensed,
    segment_fully_thought,
    segment_requires_processing,
)
from solstone.think.supervisor import is_supervisor_up
from solstone.think.talent_outputs import talent_projection_map
from solstone.think import retention_executor
from solstone.think.utils import (
    STREAM_RE,
    day_dirs,
    day_path,
    get_journal,
    segment_parse,
    segment_path,
)
from solstone.think.utils import segment_key as validate_segment_key

logger = logging.getLogger(__name__)

# Regex for YYYYMM month format validation
MONTH_RE = re.compile(r"\d{6}")
SEGMENT_DELETE_TTL = 10.0

transcripts_bp = Blueprint(
    "app:transcripts",
    __name__,
    url_prefix="/app/transcripts",
)


def _day_range_count(day: str, day_dir: Path) -> int:
    """Calendar range count for one day, read-only.

    Reads the per-day ``stats.json`` cache via the stats routine's shared
    freshness primitive; on a fresh hit returns
    ``transcript_ranges + percept_ranges + browser_segments``. On
    miss/stale/corrupt/old-schema it falls back to a fresh raw segment scan for
    THIS day only, counting the same three components (audio ranges + screen
    ranges + browser segments) so both paths stay in parity. Never writes,
    deletes, or mtime-touches ``stats.json``.
    """
    payload = load_fresh_day_cache(day_dir)
    if payload is not None:
        day_stats = payload["stats"]
        return (
            int(day_stats["transcript_ranges"])
            + int(day_stats["percept_ranges"])
            + int(day_stats["browser_segments"])
        )
    audio_ranges, screen_ranges, segments = scan_day(day)
    browser_segments = sum(1 for s in segments if "browser" in s.get("types", ()))
    return len(audio_ranges) + len(screen_ranges) + browser_segments


def _attach_think_to_segments(segments: list[dict[str, Any]], day: str) -> None:
    """Annotate each segment dict in place with a per-segment ``think`` verdict.

    Reads the day's think-layer progress fold once and applies the canonical
    per-segment sense/think verdicts. Read-only: the segment dicts are freshly
    built per request by cluster.scan_day/cluster_segments (no caching), and no
    journal state is written. ``think`` is ``None`` until a segment is fully
    sensed, then ``"awaiting"`` (sensed, not yet thought) or ``"thought"``.
    """
    progress = read_segment_progress(day)
    for seg in segments:
        data_state = seg.get("data_state") or {}
        if not segment_requires_processing(seg):
            seg["think"] = None
            continue
        if not segment_fully_sensed(data_state):
            seg["think"] = None
            continue
        thought, _reason = segment_fully_thought(
            lookup_segment_progress(progress, seg["stream"], seg["key"])
        )
        seg["think"] = "thought" if thought else "awaiting"


def _attach_streams_to_ranges(
    ranges: list[tuple[str, str]],
    segments: list[dict[str, Any]],
    content_type: str,
) -> list[dict[str, Any]]:
    """Fold per-stream attribution into each (start, end) range.

    A segment contributes to a range when its half-open span overlaps the range
    and its types include ``content_type``. Streams are sorted and de-duped.
    Range state uses best-state-wins: analyzed, then analyzing, otherwise pending.
    """

    def _to_min(hhmm: str) -> int:
        h, m = hhmm.split(":")
        return int(h) * 60 + int(m)

    out: list[dict[str, Any]] = []
    for start, end in ranges:
        range_start = _to_min(start)
        range_end = _to_min(end)
        streams: set[str] = set()
        state = DataState.PENDING.value
        think: str | None = None
        for seg in segments:
            if content_type not in seg.get("types", ()):
                continue
            seg_start = _to_min(seg["start"])
            seg_end = _to_min(seg["end"])
            if seg_start < range_end and seg_end > range_start:
                streams.add(seg["stream"])
                seg_think = seg.get("think")
                if seg_think == "awaiting":
                    think = "awaiting"
                elif seg_think == "thought" and think != "awaiting":
                    think = "thought"
                modality_state = seg.get("data_state", {}).get(content_type)
                if modality_state == DataState.ANALYZED.value:
                    state = DataState.ANALYZED.value
                elif (
                    modality_state == DataState.ANALYZING.value
                    and state != DataState.ANALYZED.value
                ):
                    state = DataState.ANALYZING.value
        out.append(
            {
                "start": start,
                "end": end,
                "streams": sorted(streams),
                "state": state,
                "think": think,
            }
        )
    return out


def _segment_markdown_files(segment_dir_path: Path) -> list[Path]:
    return sorted(
        {
            path
            for pattern in ("*_transcript.md", "imported.md")
            for path in segment_dir_path.glob(pattern)
            if path.is_file()
        }
    )


def _is_markdown_only_segment(segment_dir_path: Path, stream: str) -> bool:
    if not stream.startswith("import."):
        return False
    if not _segment_markdown_files(segment_dir_path):
        return False

    for pattern in ("*audio.jsonl", "*screen.jsonl", "*_transcript.jsonl"):
        if any(path.is_file() for path in segment_dir_path.glob(pattern)):
            return False

    # Import segments deliberately retain their original source file alongside the
    # generated transcript; unlike health cards, retained raw media does not
    # disqualify the markdown-only view. Only analyzable JSONL streams do.
    return True


def _normalize_markdown_only_segments(segments: list[dict[str, Any]], day: str) -> None:
    for seg in segments:
        stream = seg.get("stream")
        key = seg.get("key")
        if not isinstance(stream, str) or not isinstance(key, str):
            continue
        try:
            seg_dir = segment_path(day, key, stream, create=False)
        except (OSError, ValueError):
            continue
        if _is_markdown_only_segment(seg_dir, stream):
            seg["types"] = ["markdown"]
            seg["data_state"] = {"markdown": DataState.ANALYZED.value}


def _attach_visible_streams_to_ranges(
    ranges: list[tuple[str, str]],
    segments: list[dict[str, Any]],
    content_type: str,
) -> list[dict[str, Any]]:
    return [
        range_payload
        for range_payload in _attach_streams_to_ranges(ranges, segments, content_type)
        if range_payload["streams"]
    ]


def _speaker_audio_sources(segment_dir_path: Path) -> list[str]:
    return sorted(
        path.stem for path in segment_dir_path.glob("*audio.jsonl") if path.is_file()
    )


def _speaker_embedding_sources(segment_dir_path: Path) -> list[str]:
    return sorted(
        path.stem
        for path in segment_dir_path.glob("*.npz")
        if path.is_file() and (path.stem == "audio" or path.stem.endswith("_audio"))
    )


def _resolve_speaker_labels_source(
    segment_dir_path: Path,
    audio_sources: list[str],
) -> tuple[str | None, bool]:
    embedding_sources = _speaker_embedding_sources(segment_dir_path)
    if embedding_sources:
        return embedding_sources[0], False
    if len(audio_sources) == 1:
        return audio_sources[0], False
    if len(audio_sources) > 1:
        return None, True
    return None, False


def _speaker_confidence_state(confidence: Any) -> str:
    if confidence in {"high", "medium"}:
        return str(confidence)
    return "unknown"


def _speaker_labels_warning_detail(path: Path, message: str) -> dict[str, str]:
    return {
        "type": "speaker_labels",
        "file": str(path),
        "message": message,
        "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
    }


def _load_embedding_statement_ids(
    segment_dir_path: Path,
    speaker_source: str,
) -> set[int]:
    try:
        data = load_npz(segment_dir_path / f"{speaker_source}.npz")
    except Exception:
        logger.debug(
            "Failed to load speaker embeddings for source %s",
            speaker_source,
            exc_info=True,
        )
        return set()
    if not data:
        return set()

    embeddings = data.get("embeddings")
    statement_ids = data.get("statement_ids")
    if embeddings is None or statement_ids is None:
        return set()

    try:
        row_count = len(embeddings)
        raw_ids = list(statement_ids)
    except TypeError:
        return set()

    embedding_statement_ids: set[int] = set()
    for raw_id in raw_ids[:row_count]:
        try:
            embedding_statement_ids.add(int(raw_id))
        except (TypeError, ValueError):
            continue
    return embedding_statement_ids


@transcripts_bp.route("/")
def index() -> Any:
    """Redirect to the most recent day with segments, falling back to today."""
    today = date.today().strftime("%Y%m%d")
    for day in sorted(day_dirs().keys(), reverse=True):
        if cluster_segments(day):
            return redirect(url_for("app:transcripts.transcripts_day", day=day))
    return redirect(url_for("app:transcripts.transcripts_day", day=today))


@transcripts_bp.route("/api/index")
def api_index() -> Any:
    """Return read-only whole-journal date navigation coverage.

    Reuses ``_day_range_count`` for each day, so month totals match the sum of
    ``/api/stats/{month}`` for the same month.
    """
    day_counts = {
        day_name: _day_range_count(day_name, Path(path))
        for day_name, path in day_dirs().items()
    }
    return jsonify(build_date_nav_index(day_counts))


@transcripts_bp.route("/<day>")
def transcripts_day(day: str) -> Any:
    """Serve the transcript SPA shell for a specific day."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    return current_app.send_static_file("shell.html")


@transcripts_bp.route("/api/ranges/<day>")
def transcript_ranges(day: str) -> Any:
    """Return available transcript ranges for a day."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    audio_ranges, screen_ranges, segments = scan_day(day)
    _normalize_markdown_only_segments(segments, day)
    _attach_think_to_segments(segments, day)
    return jsonify(
        {
            "audio": _attach_visible_streams_to_ranges(audio_ranges, segments, "audio"),
            "screen": _attach_visible_streams_to_ranges(
                screen_ranges, segments, "screen"
            ),
        }
    )


@transcripts_bp.route("/api/segments/<day>")
def transcript_segments(day: str) -> Any:
    """Return individual recording segments for a day.

    Returns list of segments with their content types for the segment selector UI.
    """
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    segments = cluster_segments(day)
    _normalize_markdown_only_segments(segments, day)
    _attach_think_to_segments(segments, day)
    return jsonify({"segments": segments})


@transcripts_bp.route("/api/day/<day>")
def transcript_day_data(day: str) -> Any:
    """Return combined ranges and segments for a day in a single response."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    audio_ranges, screen_ranges, segments = scan_day(day)
    _normalize_markdown_only_segments(segments, day)
    _attach_think_to_segments(segments, day)
    return jsonify(
        {
            "audio": _attach_visible_streams_to_ranges(audio_ranges, segments, "audio"),
            "screen": _attach_visible_streams_to_ranges(
                screen_ranges, segments, "screen"
            ),
            "segments": segments,
        }
    )


@transcripts_bp.route("/api/read/<day>")
def api_read(day: str) -> Any:
    """Return clustered transcript markdown for a day, segment, span, or range."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    sources: dict[str, bool] = {
        "transcripts": request.args.get("transcripts") == "1",
        "percepts": request.args.get("percepts") == "1",
        "agents": request.args.get("agents") == "1",
    }
    start = request.args.get("start")
    end = request.args.get("end")
    segment = request.args.get("segment")
    segments = request.args.get("segments")
    stream = request.args.get("stream")

    if start and end:
        markdown = cluster_range(day, start, end, sources)
    elif segments:
        span = [s.strip() for s in segments.split(",") if s.strip()]
        try:
            markdown, _counts = cluster_span(day, span, sources, stream=stream)
        except ValueError as exc:
            return error_response(INVALID_SEGMENT_OR_STREAM, detail=str(exc))
    elif segment:
        markdown, _counts = cluster_period(day, segment, sources, stream=stream)
    else:
        markdown, _counts = cluster(day, sources)

    return jsonify({"markdown": markdown})


@transcripts_bp.route("/api/serve_file/<day>/<path:rel_path>")
def serve_file(day: str, rel_path: str) -> Any:
    """Serve actual media files for embedding."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")
    path, error = safe_day_path(day, rel_path)
    if error is not None:
        return error
    if not path.is_file():
        return error_response(FILE_NOT_FOUND, detail="File not found")
    mimetype = MIME_TYPES.get(path.suffix.lower())
    if mimetype is None:
        raise ValueError(f"unregistered media extension for serve_file: {path.suffix}")
    return send_file(path, conditional=True, mimetype=mimetype)


@transcripts_bp.route("/api/stats/<month>")
def api_stats(month: str):
    """Return transcript range counts for each day in a specific month.

    Args:
        month: YYYYMM format month string

    Returns:
        JSON dict mapping day (YYYYMMDD) to transcript range count, zero-count
        days omitted. Counts are served from the per-day ``stats.json`` cache
        written by the journal-stats routine, falling back to a raw cluster scan
        per day only when that day's cache is missing or stale. Transcripts app
        is not facet-aware, so returns a simple {day: count} mapping.
    """
    if not MONTH_RE.fullmatch(month):
        return error_response(INVALID_MONTH, detail="Invalid month format")

    stats: dict[str, int] = {}
    for day_name, path in day_dirs().items():
        if not day_name.startswith(month):
            continue
        count = _day_range_count(day_name, Path(path))
        if count > 0:
            stats[day_name] = count
    return jsonify(stats)


def _load_jsonl(path: str) -> list[dict]:
    """Load JSONL file and return list of entries."""
    import json

    entries = []
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if line:
                entries.append(json.loads(line))
    return entries


def _format_time_from_offset(segment_key: str, offset_sec: float) -> str:
    """Convert segment start + offset to HH:MM:SS format."""
    start_time, _ = segment_parse(segment_key)
    if not start_time:
        return ""

    total_sec = start_time.hour * 3600 + start_time.minute * 60 + start_time.second
    total_sec += int(offset_sec)

    h = total_sec // 3600
    m = (total_sec % 3600) // 60
    s = total_sec % 60
    return f"{h:02d}:{m:02d}:{s:02d}"


def _timestamp_from_day_time(day: str, time_str: str, fallback: int = 0) -> int:
    """Convert a journal day plus wall-clock HH:MM:SS to local unix ms."""
    if not time_str:
        return fallback
    try:
        dt = datetime.strptime(f"{day} {time_str}", "%Y%m%d %H:%M:%S")
    except ValueError:
        return fallback
    return int(dt.timestamp() * 1000)


def _timestamp_from_value(value: Any) -> int:
    """Convert an ISO timestamp or epoch-ish value to unix ms."""
    if isinstance(value, int | float):
        return int(value if value > 10_000_000_000 else value * 1000)
    if not isinstance(value, str) or not value:
        return 0
    try:
        normalized = value.replace("Z", "+00:00")
        return int(datetime.fromisoformat(normalized).timestamp() * 1000)
    except ValueError:
        return 0


def _local_time_from_timestamp(timestamp_ms: int) -> str:
    if timestamp_ms <= 0:
        return ""
    return datetime.fromtimestamp(timestamp_ms / 1000).strftime("%H:%M:%S")


def _first_browser_segment_start(entries: list[dict]) -> dict[str, Any] | None:
    for entry in entries:
        if entry.get("t") == "segment_start":
            return entry
    return None


def _browser_site_name(filename: str, segment_start: dict[str, Any] | None) -> str:
    segment_start = segment_start or {}
    adapter = str(segment_start.get("adapter") or "").strip()
    if adapter:
        return adapter.title()
    site = str(segment_start.get("site") or "").strip()
    if site:
        return site
    stem = filename
    if stem.startswith("browser_"):
        stem = stem[len("browser_") :]
    if stem.endswith(".jsonl"):
        stem = stem[: -len(".jsonl")]
    return stem.replace("-", ".") or filename


def _load_segment_signals(segment_dir_path: Path) -> dict[str, Any]:
    """Load optional Mentra signal context for display in the transcript app."""
    signals_path = segment_dir_path / "signals.jsonl"
    if not signals_path.is_file():
        return {
            "events": [],
            "counts": {},
            "calendar": {"total": 0, "unique": 0, "events": []},
        }

    events: list[dict[str, Any]] = []
    counts: dict[str, int] = {}
    calendar_by_key: dict[tuple[Any, ...], dict[str, Any]] = {}
    try:
        records = _load_jsonl(str(signals_path))
    except (OSError, json.JSONDecodeError):
        logger.debug("Failed to read segment signals %s", signals_path, exc_info=True)
        records = []

    for record in records:
        event_type = record.get("event_type")
        if not isinstance(event_type, str) or not event_type:
            continue
        payload = record.get("payload")
        payload = payload if isinstance(payload, dict) else {}
        timestamp = (
            record.get("timestamp")
            or payload.get("timestamp")
            or payload.get("timeStamp")
        )
        timestamp_ms = _timestamp_from_value(timestamp)
        event = {
            "event_type": event_type,
            "time": _local_time_from_timestamp(timestamp_ms),
            "timestamp": timestamp if isinstance(timestamp, str) else "",
            "timestamp_ms": timestamp_ms,
            "payload": payload,
        }
        events.append(event)
        counts[event_type] = counts.get(event_type, 0) + 1

        if event_type == "calendar_event":
            key = (
                payload.get("eventId"),
                payload.get("title"),
                payload.get("dtStart"),
                payload.get("dtEnd"),
            )
            calendar = calendar_by_key.get(key)
            if calendar is None:
                calendar = {
                    "title": payload.get("title") or "Untitled event",
                    "dtStart": payload.get("dtStart") or "",
                    "dtEnd": payload.get("dtEnd") or "",
                    "timezone": payload.get("timezone") or "",
                    "eventId": payload.get("eventId") or "",
                    "seen_count": 0,
                    "first_seen": event["timestamp"],
                    "last_seen": event["timestamp"],
                }
                calendar_by_key[key] = calendar
            calendar["seen_count"] += 1
            if event["timestamp"]:
                calendar["last_seen"] = event["timestamp"]

    events.sort(key=lambda event: (event["timestamp_ms"], event["event_type"]))
    calendar_events = sorted(
        calendar_by_key.values(),
        key=lambda event: (
            str(event.get("dtStart") or ""),
            str(event.get("title") or ""),
        ),
    )
    return {
        "events": events,
        "counts": counts,
        "calendar": {
            "total": counts.get("calendar_event", 0),
            "unique": len(calendar_events),
            "events": calendar_events,
        },
    }


def _read_audio_duration_seconds(entries: list[dict], segment_key: str) -> float:
    """Best-effort segment audio duration in seconds (read-only).

    Prefers the transcribe-time `duration` from the audio header entry (the
    metadata entry without a `start`); falls back to the segment-key window
    length (HHMMSS_LEN). Returns 0.0 if neither is available.
    """
    for entry in entries:
        if "start" in entry:
            continue
        duration = entry.get("duration")
        try:
            duration_seconds = float(duration)
        except (TypeError, ValueError):
            continue
        if duration_seconds > 0:
            return duration_seconds

    start_time, end_time = segment_parse(segment_key)
    if not start_time or not end_time:
        return 0.0

    start_seconds = start_time.hour * 3600 + start_time.minute * 60 + start_time.second
    end_seconds = end_time.hour * 3600 + end_time.minute * 60 + end_time.second
    window_seconds = end_seconds - start_seconds
    if window_seconds > 0:
        return float(window_seconds)
    return 0.0


def _analyzing_marker_path(segment_dir_path: Path, modality: str) -> Path:
    return segment_dir_path / f".analyzing_{modality}"


def _analyze_failed_marker_path(segment_dir_path: Path, modality: str) -> Path:
    return segment_dir_path / f".analyze_failed_{modality}"


def _read_marker_payload(marker_path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(marker_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return payload if isinstance(payload, dict) else {}


def _segment_modality_signals(
    segment_dir_path: Path, modality: str
) -> dict[str, bool | str]:
    extensions = AUDIO_EXTENSIONS if modality == "audio" else VIDEO_EXTENSIONS
    has_raw_present = any(
        path.is_file() and path.suffix.lower() in extensions
        for path in segment_dir_path.iterdir()
    )
    has_raw_reference = False
    has_raw_file = False
    has_jsonl = False
    has_chunks = False
    record: dict | None = None
    warning = False

    patterns = ("*audio.jsonl",) if modality == "audio" else ("*screen.jsonl",)
    for pattern in patterns:
        for jsonl_path in sorted(segment_dir_path.glob(pattern)):
            if not jsonl_path.is_file():
                continue
            has_jsonl = True
            try:
                entries = _load_jsonl(str(jsonl_path))
                record = record or read_processing_record(entries)
                if modality == "audio":
                    formatted_chunks, _meta = format_audio(
                        entries, {"file_path": str(jsonl_path)}
                    )
                    for entry in entries:
                        if "start" not in entry and "raw" in entry:
                            raw_name = entry["raw"]
                            if raw_name.endswith(AUDIO_EXTENSIONS):
                                has_raw_reference = True
                                has_raw_file = (segment_dir_path / raw_name).is_file()
                            break
                else:
                    formatted_chunks, _meta = format_screen(
                        entries, {"file_path": str(jsonl_path)}
                    )
                    for entry in entries:
                        if "frame_id" not in entry and "raw" in entry:
                            raw_name = entry["raw"]
                            if raw_name.endswith(VIDEO_EXTENSIONS):
                                has_raw_reference = True
                                has_raw_file = (segment_dir_path / raw_name).is_file()
                            break
                has_chunks = has_chunks or bool(formatted_chunks)
            except Exception:
                warning = True

    media_purged = has_raw_reference and not has_raw_file
    if has_chunks:
        state = derive_modality_state(
            segment_dir_path,
            modality,
            has_chunks=True,
            has_jsonl=has_jsonl,
            has_raw=has_raw_present,
            record=record,
        )
    elif media_purged:
        state = DataState.PURGED.value
    else:
        state = derive_modality_state(
            segment_dir_path,
            modality,
            has_chunks=False,
            has_jsonl=has_jsonl,
            has_raw=has_raw_present,
            record=record,
        )
        if warning and state == DataState.PENDING.value:
            state = DataState.FAILED.value

    return {
        "state": state,
        "has_raw": has_raw_present,
        "has_jsonl": has_jsonl,
        "has_chunks": has_chunks,
        "media_purged": media_purged,
    }


def _segment_data_state(segment_dir_path: Path) -> dict[str, str]:
    data_state: dict[str, str] = {}
    for modality in ("audio", "screen"):
        state = str(_segment_modality_signals(segment_dir_path, modality)["state"])
        if state != DataState.ABSENT.value:
            data_state[modality] = state
    return data_state


def _write_failed_reprocess_marker(
    marker_path: Path,
    failed_path: Path,
    reason: str,
    detail: str,
    reason_code: str | None = None,
) -> None:
    marker_payload = _read_marker_payload(marker_path)
    payload = {
        "started_at": marker_payload.get("started_at", ""),
        "modality": marker_payload.get("modality", ""),
        "reason": reason,
        "failed_at": datetime.utcnow().isoformat(timespec="seconds") + "Z",
        "detail": detail,
    }
    if reason_code is not None:
        payload["reason_code"] = reason_code
    tmp = failed_path.with_suffix(failed_path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(failed_path)
    marker_path.unlink(missing_ok=True)


def _watch_reprocess_completion(
    proc: subprocess.Popen,
    marker_path: Path,
    failed_path: Path,
    segment_dir_path: Path,
    modality: str,
    request_id: str,
) -> None:
    try:
        rc = proc.wait()
        stderr_tail = ""
        if proc.stderr:
            stderr_tail = (proc.stderr.read() or b"")[-512:].decode("utf-8", "replace")
        marker_payload = _read_marker_payload(marker_path)
        if marker_payload.get("request_id") != request_id:
            return
        if rc == 0:
            state = str(_segment_modality_signals(segment_dir_path, modality)["state"])
            if state in {DataState.ANALYZED.value, DataState.EMPTY.value}:
                marker_path.unlink(missing_ok=True)
                return
            _write_failed_reprocess_marker(
                marker_path,
                failed_path,
                "no_output",
                "worker exited 0 without analyzed chunks",
                reason_code="no_output",
            )
            return
        _write_failed_reprocess_marker(
            marker_path,
            failed_path,
            f"exit_{rc}",
            stderr_tail,
        )
    except Exception:
        logger.exception("reprocess watcher failed")


@transcripts_bp.route("/api/segment/<day>/<stream>/<segment_key>")
def segment_content(day: str, stream: str, segment_key: str) -> Any:
    """Return unified timeline of audio and screen entries for a segment.

    Uses format_audio() and format_screen() to get chunks with source data,
    then merges chronologically for unified display.

    Returns JSON with:
        - chunks: List of entries sorted by timestamp, each with:
            - type: "audio" or "screen"
            - time: formatted wall-clock time (HH:MM:SS)
            - timestamp: unix ms for ordering
            - markdown: formatted content
            - source_ref: key fields from source for media lookup
        - audio_file: URL to segment audio file (if exists)
        - video_files: dict mapping jsonl filename to video URL for client-side decoding
        - image_files: dict mapping image filename to image URL for still-image frames
        - segment_key: segment directory name
        - cost: processing cost in USD (float, 0.0 if no data)
        - media_sizes: dict with audio/screen byte counts for raw media files
        - media_purged: dict with audio/screen raw-reference purge flags
        - data_state: dict of advertised modality states
        - signals: optional Mentra signal sidecar context from signals.jsonl
    """
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Invalid day format")

    if not STREAM_RE.fullmatch(stream):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=404,
            detail="Invalid stream format",
        )

    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=404,
            detail="Invalid segment key format",
        )

    segment_dir_path = segment_path(day, segment_key, stream, create=False)
    segment_dir = str(segment_dir_path)
    if not segment_dir_path.is_dir():
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=404,
            detail="Segment directory not found",
        )

    chunks: list[dict] = []
    audio_file_url = None
    audio_duration = 0.0
    video_files: dict[str, str] = {}  # jsonl filename -> video URL
    image_files: dict[str, str] = {}  # raw image filename -> image URL
    media_sizes: dict[str, int] = {"audio": 0, "screen": 0}
    has_raw_reference = {"audio": False, "screen": False}
    has_raw_file = {"audio": False, "screen": False}
    has_raw_present = {"audio": False, "screen": False}
    has_jsonl = {"audio": False, "screen": False}
    processing_records: dict[str, dict | None] = {"audio": None, "screen": None}
    counted_media_paths: set[Path] = set()
    warning_details: list[dict[str, str]] = []

    markdown_only_segment = _is_markdown_only_segment(segment_dir_path, stream)
    if not markdown_only_segment:
        for raw_media in sorted(segment_dir_path.iterdir()):
            if not raw_media.is_file():
                continue
            suffix = raw_media.suffix.lower()
            if suffix in AUDIO_EXTENSIONS:
                has_raw_present["audio"] = True
                counted_media_paths.add(raw_media.resolve())
                media_sizes["audio"] += raw_media.stat().st_size
            elif suffix in VIDEO_EXTENSIONS or suffix in IMAGE_EXTENSIONS:
                has_raw_present["screen"] = True
                counted_media_paths.add(raw_media.resolve())
                media_sizes["screen"] += raw_media.stat().st_size

    # Load speaker labels if available.
    speaker_labels_path = segment_dir_path / "talents" / "speaker_labels.json"
    speaker_labels_present = speaker_labels_path.is_file()
    speaker_labels_loaded = False
    audio_sources = _speaker_audio_sources(segment_dir_path)
    labels_source, labels_ambiguous = _resolve_speaker_labels_source(
        segment_dir_path,
        audio_sources,
    )
    speaker_map: dict[int, dict] = {}
    if speaker_labels_present and labels_ambiguous:
        warning_details.append(
            _speaker_labels_warning_detail(
                speaker_labels_path,
                SPEAKER_LABEL_SOURCE_AMBIGUOUS_MESSAGE,
            )
        )
    if speaker_labels_present:
        try:
            with open(speaker_labels_path) as f:
                labels_data = json.load(f)
            if not isinstance(labels_data, dict):
                raise ValueError("speaker labels payload must be an object")
            principal = get_journal_principal()
            principal_id = principal["id"] if principal else None
            entity_cache: dict[str, dict | None] = {}
            labels = labels_data.get("labels", [])
            if not isinstance(labels, list):
                raise ValueError("speaker labels must be a list")
            speaker_labels_loaded = True
            for label in labels:
                if not isinstance(label, dict):
                    continue
                sid = label.get("sentence_id")
                entity_id = label.get("speaker")
                confidence = label.get("confidence")
                if sid is None or not entity_id:
                    continue
                try:
                    sentence_id = int(sid)
                except (TypeError, ValueError):
                    continue
                if entity_id not in entity_cache:
                    entity_cache[entity_id] = load_journal_entity(entity_id)
                entity = entity_cache[entity_id]
                name = entity["name"] if entity else entity_id
                is_owner = entity_id == principal_id
                speaker_map[sentence_id] = {
                    "name": name,
                    "entity_id": entity_id,
                    "confidence": confidence,
                    "confidence_state": _speaker_confidence_state(confidence),
                    "method": label.get("method"),
                    "owner_margin_declined": bool(label.get("owner_margin_declined")),
                    "acoustic_margin_declined": bool(
                        label.get("acoustic_margin_declined")
                    ),
                    "is_owner": is_owner,
                }
        except (json.JSONDecodeError, OSError, KeyError, TypeError, ValueError):
            logger.warning(
                "Failed to load speaker labels %s",
                speaker_labels_path,
                exc_info=True,
            )
            warning_details.append(
                _speaker_labels_warning_detail(
                    speaker_labels_path,
                    SPEAKER_LABELS_UNAVAILABLE_MESSAGE,
                )
            )

    # Process audio files
    audio_files = glob(os.path.join(segment_dir, "*audio.jsonl"))
    for audio_path in sorted(audio_files):
        has_jsonl["audio"] = True
        speaker_source = Path(audio_path).stem
        embedding_statement_ids = _load_embedding_statement_ids(
            segment_dir_path,
            speaker_source,
        )
        try:
            entries = _load_jsonl(audio_path)
            record = read_processing_record(entries)
            processing_records["audio"] = processing_records["audio"] or record
            audio_duration = max(
                audio_duration,
                _read_audio_duration_seconds(entries, segment_key),
            )
            formatted_chunks, meta = format_audio(entries, {"file_path": audio_path})

            # Build sentence_id mapping (1-based over transcript entries only).
            entry_to_sid: dict[int, int] = {}
            sid = 0
            for entry in entries:
                if "start" in entry:
                    sid += 1
                    entry_to_sid[id(entry)] = sid

            # Find the raw audio file from metadata (first entry without "start")
            raw_audio = None
            for entry in entries:
                if "start" not in entry and "raw" in entry:
                    raw_audio = entry["raw"]
                    break

            # Validate raw points to an audio file (skip if not)
            if raw_audio and raw_audio.endswith(AUDIO_EXTENSIONS):
                has_raw_reference["audio"] = True
                audio_full = segment_dir_path / raw_audio
                if audio_full.is_file():
                    has_raw_present["audio"] = True
                    has_raw_file["audio"] = True
                    rel_path = f"{stream}/{segment_key}/{raw_audio}"
                    audio_file_url = f"/app/transcripts/api/serve_file/{day}/{rel_path}"
                    resolved = audio_full.resolve()
                    if resolved not in counted_media_paths:
                        counted_media_paths.add(resolved)
                        media_sizes["audio"] += audio_full.stat().st_size

            for chunk in formatted_chunks:
                source = chunk.get("source", {})
                # Audio has start time in HH:MM:SS format
                time_str = source.get("start", "")
                markdown = chunk.get("markdown", "")
                markdown = re.sub(r"^\[\d{2}:\d{2}:\d{2}\]\s*", "", markdown)
                speaker_token = source.get("speaker")
                if speaker_token is not None:
                    if isinstance(speaker_token, int):
                        markdown = re.sub(
                            r"Speaker\s+\d+:\s*",
                            "",
                            markdown,
                            count=1,
                        )
                    else:
                        markdown = re.sub(
                            rf"{re.escape(str(speaker_token))}:\s*",
                            "",
                            markdown,
                            count=1,
                        )

                chunk_sid = entry_to_sid.get(id(source))
                speaker_label = (
                    speaker_map.get(chunk_sid)
                    if chunk_sid and labels_source == speaker_source
                    else None
                )

                chunk_data: dict[str, Any] = {
                    "type": "audio",
                    "time": time_str,
                    # NOTE: audio `start` is an offset from segment start per the
                    # journal format contract; format_audio anchors it correctly.
                    # Do not reinterpret it as wall-clock here — writers that emit
                    # wall-clock starts (early mentra bridge) fix it at the writer.
                    "timestamp": chunk.get("timestamp", 0),
                    "markdown": markdown,
                    "sentence_id": chunk_sid,
                    "speaker_source": speaker_source,
                    "has_embedding": bool(
                        chunk_sid and chunk_sid in embedding_statement_ids
                    ),
                    "speaker_actionable": bool(
                        speaker_labels_present
                        and speaker_labels_loaded
                        and labels_source == speaker_source
                        and chunk_sid
                        and chunk_sid in embedding_statement_ids
                    ),
                    "source_ref": {
                        "start": time_str,
                        "source": source.get("source"),
                        "speaker": source.get("speaker"),
                    },
                }
                if speaker_label:
                    chunk_data["speaker_label"] = speaker_label
                chunks.append(chunk_data)
        except Exception as exc:
            logger.warning(
                "Failed to parse audio segment %s", audio_path, exc_info=True
            )
            warning_details.append(
                {
                    "type": "audio",
                    "file": str(audio_path),
                    "message": str(exc),
                    "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
                }
            )
            continue

    # Process screen files and collect video URLs for client-side decoding
    screen_files = glob(os.path.join(segment_dir, "*screen.jsonl"))
    for screen_path in sorted(screen_files):
        has_jsonl["screen"] = True
        try:
            entries = _load_jsonl(screen_path)
            record = read_processing_record(entries)
            processing_records["screen"] = processing_records["screen"] or record

            filename = os.path.basename(screen_path)
            monitor = (
                filename.replace("_screen.jsonl", "")
                if filename != "screen.jsonl"
                else ""
            )

            image_candidates = [
                path.name
                for path in sorted(segment_dir_path.iterdir())
                if path.is_file() and path.suffix.lower() in IMAGE_EXTENSIONS
            ]

            # Extract raw media from header (first entry without frame_id).
            raw_media = None
            for entry in entries:
                if "frame_id" not in entry and "raw" in entry:
                    raw_media = entry["raw"]
                    break

            def register_screen_media(raw_name: Any) -> str | None:
                if not isinstance(raw_name, str):
                    return None
                suffix = Path(raw_name).suffix.lower()
                if suffix not in VIDEO_EXTENSIONS and suffix not in IMAGE_EXTENSIONS:
                    return None
                has_raw_reference["screen"] = True
                media_full = segment_dir_path / raw_name
                if not media_full.is_file():
                    return None
                has_raw_present["screen"] = True
                has_raw_file["screen"] = True
                rel_path = f"{stream}/{segment_key}/{raw_name}"
                media_url = f"/app/transcripts/api/serve_file/{day}/{rel_path}"
                if suffix in VIDEO_EXTENSIONS:
                    video_files[filename] = media_url
                else:
                    image_files[raw_name] = media_url
                resolved = media_full.resolve()
                if resolved not in counted_media_paths:
                    counted_media_paths.add(resolved)
                    media_sizes["screen"] += media_full.stat().st_size
                return "video" if suffix in VIDEO_EXTENSIONS else "image"

            def image_description(raw_name: str | None) -> str:
                if not raw_name:
                    return ""
                sidecar = (segment_dir_path / raw_name).with_suffix(".jsonl")
                if not sidecar.is_file():
                    return ""
                try:
                    for record in _load_jsonl(str(sidecar)):
                        text = record.get("text")
                        if isinstance(text, str) and text.strip():
                            return text.strip()
                except Exception:
                    logger.debug(
                        "Failed to read image sidecar %s", sidecar, exc_info=True
                    )
                return ""

            register_screen_media(raw_media)

            enriched_entries = []
            frame_index = 0
            for entry in entries:
                if "timestamp" not in entry:
                    enriched_entries.append(entry)
                    continue

                frame = dict(entry)
                content_value = frame.get("content")
                frame_content = (
                    dict(content_value) if isinstance(content_value, dict) else {}
                )
                # content["media"] is photo metadata only when it's a dict (the
                # external mentra-photo observer). For a "media"-category describe
                # frame it is a markdown string — leave it untouched and treat the
                # frame as carrying no photo metadata.
                media_value = frame_content.get("media")
                had_media_key = "media" in frame_content
                is_photo_dict = isinstance(media_value, dict)
                media_content = dict(media_value) if is_photo_dict else {}
                frame_raw = (
                    frame.get("raw")
                    or media_content.get("photo_file")
                    or media_content.get("photo_filename")
                    or media_content.get("photo_name")
                )

                if not frame_raw and image_candidates:
                    if len(image_candidates) == 1:
                        frame_raw = image_candidates[0]
                    elif frame_index < len(image_candidates):
                        frame_raw = image_candidates[frame_index]
                if not frame_raw:
                    frame_raw = raw_media

                media_kind = register_screen_media(frame_raw)
                if media_kind == "image" and isinstance(frame_raw, str):
                    frame["raw"] = frame_raw
                    description = image_description(frame_raw)
                    media_content.setdefault("photo_file", frame_raw)
                    if description and not media_content.get("description"):
                        media_content["description"] = description
                    # Only persist synthesized photo metadata for genuine photo
                    # frames (media was a dict) or frames with no "media" key —
                    # never clobber a "media"-category markdown string.
                    if is_photo_dict or not had_media_key:
                        frame_content["media"] = media_content
                        frame["content"] = frame_content
                    if description:
                        analysis_value = frame.get("analysis")
                        analysis = (
                            dict(analysis_value)
                            if isinstance(analysis_value, dict)
                            else {}
                        )
                        existing_description = str(
                            analysis.get("visual_description") or ""
                        ).strip()
                        if existing_description in {
                            "",
                            "Mentra Live photo captured.",
                            "Photo capture",
                        }:
                            analysis["visual_description"] = description
                        frame["analysis"] = analysis
                else:
                    register_screen_media(raw_media)

                enriched_entries.append(frame)
                frame_index += 1

            formatted_chunks, meta = format_screen(
                enriched_entries,
                {"file_path": screen_path},
            )

            for chunk in formatted_chunks:
                source = chunk.get("source", {})
                frame_id = source.get("frame_id")
                offset = source.get("timestamp", 0)
                source_raw = source.get("raw") or raw_media
                source_suffix = (
                    Path(source_raw).suffix.lower()
                    if isinstance(source_raw, str)
                    else ""
                )
                media_kind = (
                    "image"
                    if source_suffix in IMAGE_EXTENSIONS
                    else "video"
                    if source_suffix in VIDEO_EXTENSIONS
                    else None
                )

                # Calculate wall-clock time from segment start + offset
                time_str = _format_time_from_offset(segment_key, offset)

                # Basic frames have no enriched content
                frame_content = source.get("content", {})
                is_basic = not frame_content

                # Extract participant boxes for meeting frames
                participants = []
                meeting_data = frame_content.get("meeting")
                if isinstance(meeting_data, dict):
                    for p in meeting_data.get("participants", []):
                        if not isinstance(p, dict):
                            continue
                        box = p.get("box_2d")
                        # Only include participants with video and valid box_2d
                        if p.get("video") and box and len(box) == 4:
                            y_min, x_min, y_max, x_max = box
                            participants.append(
                                {
                                    "name": p.get("name", "Unknown"),
                                    "status": p.get("status", "unknown"),
                                    "top": y_min / 10,
                                    "left": x_min / 10,
                                    "height": (y_max - y_min) / 10,
                                    "width": (x_max - x_min) / 10,
                                }
                            )

                # Include box_2d for client-side bounding box drawing
                box_2d = source.get("box_2d")

                chunks.append(
                    {
                        "type": "screen",
                        "time": time_str,
                        "timestamp": _timestamp_from_day_time(
                            day, time_str, chunk.get("timestamp", 0)
                        ),
                        "markdown": chunk.get("markdown", ""),
                        "source_ref": {
                            "frame_id": frame_id,
                            "filename": filename,
                            "raw": source_raw,
                            "media_kind": media_kind,
                            "monitor": monitor,
                            "offset": offset,
                            "box_2d": box_2d,
                            "analysis": source.get("analysis"),
                            "participants": participants if participants else None,
                            "aruco": source.get("aruco"),
                        },
                        "basic": is_basic,
                    }
                )
        except Exception as exc:
            logger.warning(
                "Failed to parse screen segment %s", screen_path, exc_info=True
            )
            warning_details.append(
                {
                    "type": "screen",
                    "file": str(screen_path),
                    "message": str(exc),
                    "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
                }
            )
            continue

    # Process browser files. Browser timestamps are absolute epoch-ms values.
    browser_files = glob(os.path.join(segment_dir, "browser_*.jsonl"))
    for browser_path in sorted(browser_files):
        try:
            entries = _load_jsonl(browser_path)
            formatted_chunks, meta = format_browser(
                entries, {"file_path": browser_path}
            )
            if meta.get("error") and not formatted_chunks:
                raise ValueError(str(meta["error"]))

            filename = os.path.basename(browser_path)
            segment_start = _first_browser_segment_start(entries)
            site = str((segment_start or {}).get("site") or "")
            title = str((segment_start or {}).get("title") or "")
            adapter = str((segment_start or {}).get("adapter") or "")
            site_name = _browser_site_name(filename, segment_start)

            for chunk in formatted_chunks:
                source = chunk.get("source", {})
                timestamp = int(chunk.get("timestamp") or 0)
                chunks.append(
                    {
                        "type": "browser",
                        "time": _local_time_from_timestamp(timestamp),
                        "timestamp": timestamp,
                        "markdown": chunk.get("markdown", ""),
                        "source_ref": {
                            "site": site,
                            "title": title,
                            "adapter": adapter,
                            "site_name": site_name,
                            "file": filename,
                            "op": source.get("op") or source.get("t"),
                        },
                    }
                )
        except Exception as exc:
            logger.warning(
                "Failed to parse browser segment %s", browser_path, exc_info=True
            )
            warning_details.append(
                {
                    "type": "browser",
                    "file": str(browser_path),
                    "message": str(exc),
                    "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
                }
            )
            continue

    markdown_chunks_added = False
    if markdown_only_segment:
        time_str = _format_time_from_offset(segment_key, 0)
        timestamp = _timestamp_from_day_time(day, time_str)
        for md_path in _segment_markdown_files(segment_dir_path):
            try:
                markdown = md_path.read_text(encoding="utf-8").strip()
            except OSError as exc:
                logger.warning(
                    "Failed to read markdown segment %s", md_path, exc_info=True
                )
                warning_details.append(
                    {
                        "type": "markdown",
                        "file": str(md_path),
                        "message": str(exc),
                        "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
                    }
                )
                continue
            if not markdown:
                continue
            chunks.append(
                {
                    "type": "markdown",
                    "time": time_str,
                    "timestamp": timestamp,
                    "markdown": markdown,
                    "source_ref": {"filename": md_path.name},
                }
            )
            markdown_chunks_added = True

    # Sort all chunks by timestamp
    chunks.sort(key=lambda c: c["timestamp"])
    media_purged = {
        modality: has_raw_reference[modality] and not has_raw_file[modality]
        for modality in ("audio", "screen")
    }
    warning_types = {
        detail["type"]
        for detail in warning_details
        if detail.get("type") in ("audio", "screen")
    }
    data_state: dict[str, str] = {}
    for modality in ("audio", "screen"):
        has_chunks = any(chunk["type"] == modality for chunk in chunks)
        if has_chunks:
            data_state[modality] = derive_modality_state(
                segment_dir_path,
                modality,
                has_chunks=True,
                has_jsonl=has_jsonl[modality],
                has_raw=has_raw_present[modality],
                record=processing_records[modality],
            )
        elif media_purged[modality]:
            data_state[modality] = DataState.PURGED.value
        else:
            state = derive_modality_state(
                segment_dir_path,
                modality,
                has_chunks=has_chunks,
                has_jsonl=has_jsonl[modality],
                has_raw=has_raw_present[modality],
                record=processing_records[modality],
            )
            if state != DataState.ABSENT.value:
                if modality in warning_types and state == DataState.PENDING.value:
                    state = DataState.FAILED.value
                data_state[modality] = state
    if markdown_chunks_added:
        data_state["markdown"] = DataState.ANALYZED.value
    if any(chunk["type"] == "browser" for chunk in chunks):
        data_state["browser"] = DataState.ANALYZED.value
    # Get cost data for this segment
    cost_data = get_usage_cost(day, segment=segment_key)

    # Collect text projections of talent outputs.
    talents_dir = segment_dir_path / "talents"
    md_files = talent_projection_map(talents_dir)

    # UI dedup: when a segment has structural modality data (screen/audio),
    # the structural tab already covers it — drop the matching talents/<mod>
    # projection so the tab row doesn't render two tabs labeled the same.
    if "screen" in data_state:
        md_files.pop("screen", None)
    if "audio" in data_state:
        md_files.pop("audio", None)

    signals = _load_segment_signals(segment_dir_path)

    return jsonify(
        {
            "chunks": chunks,
            "audio_file": audio_file_url,
            "duration": audio_duration,
            "video_files": video_files,
            "image_files": image_files,
            "md_files": md_files,
            "segment_key": segment_key,
            "cost": cost_data["cost"],
            "media_sizes": media_sizes,
            "media_purged": media_purged,
            "data_state": data_state,
            "signals": signals,
            "transcripts_copy": transcripts_copy_payload(),
            "speaker_labels": {
                "present": speaker_labels_present,
                "loaded": speaker_labels_loaded,
                "source": labels_source if speaker_labels_present else None,
                "ambiguous": bool(speaker_labels_present and labels_ambiguous),
            },
            "warnings": len(warning_details),
            "warning_details": warning_details,
        }
    )


@transcripts_bp.route(
    "/api/segment/<day>/<stream>/<segment_key>/reprocess",
    methods=["POST"],
)
def reprocess_segment(day: str, stream: str, segment_key: str) -> Any:
    """Start per-modality reprocessing for a segment."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key format",
        )

    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream format")

    day_dir = str(day_path(day, create=False))
    segment_dir_path = segment_path(day, segment_key, stream, create=False)
    segment_dir = str(segment_dir_path)

    if not os.path.isdir(day_dir):
        return error_response(
            INVALID_DAY,
            status=404,
            detail="Day not found",
        )

    if not os.path.isdir(segment_dir):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=404,
            detail="Segment not found",
        )

    if not os.path.commonpath([segment_dir, day_dir]) == day_dir:
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=403,
            detail="Invalid segment path",
        )

    body = request.get_json(silent=True)
    modality = body.get("modality") if isinstance(body, dict) else None
    if modality not in {"audio", "screen"}:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="modality must be audio or screen",
        )

    signals = _segment_modality_signals(segment_dir_path, modality)
    state = str(signals["state"])
    has_raw = bool(signals["has_raw"])
    if state == DataState.ANALYZED.value:
        return error_response(
            INVALID_OPERATION_FOR_STATE,
            detail="Segment modality is already analyzed",
        )
    if state == DataState.PURGED.value or not has_raw:
        return error_response(
            RAW_MEDIA_NOT_AVAILABLE,
            detail="Raw media is no longer available",
        )
    marker_path = _analyzing_marker_path(segment_dir_path, modality)
    failed_path = _analyze_failed_marker_path(segment_dir_path, modality)
    if state == DataState.ANALYZING.value:
        data_state = _segment_data_state(segment_dir_path)
        data_state[modality] = DataState.ANALYZING.value
        marker = _read_marker_payload(marker_path)
        return jsonify(
            {
                "data_state": data_state,
                "marker": {"started_at": marker.get("started_at", "")},
                "repair_status": "running",
            }
        )

    if state in {DataState.FAILED.value, DataState.FAILED_FINAL.value}:
        # Reprocess deletes the jsonl; clear stale failed markers before they
        # can out-rank a legitimately re-running segment.
        repair_modality_markers(
            segment_dir_path,
            modality,
            has_chunks=bool(signals["has_chunks"]),
            has_jsonl=bool(signals["has_jsonl"]),
            has_raw=has_raw,
        )
        failed_path.unlink(missing_ok=True)

    try:
        marker_path = create_analyzing_marker(segment_dir_path, modality)
    except FileExistsError:
        data_state = _segment_data_state(segment_dir_path)
        data_state[modality] = DataState.ANALYZING.value
        marker = _read_marker_payload(marker_path)
        return jsonify(
            {
                "data_state": data_state,
                "marker": {"started_at": marker.get("started_at", "")},
                "repair_status": "running",
            }
        )
    marker = _read_marker_payload(marker_path)
    request_id = str(marker.get("request_id", ""))

    argv = [
        sys.executable,
        "-m",
        "solstone.observe.sense",
        "--day",
        day,
        "--segment",
        segment_key,
        "--stream",
        stream,
        "--reprocess",
        modality,
    ]
    try:
        proc = subprocess.Popen(
            argv,
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        marker_path.unlink(missing_ok=True)
        return error_response(
            FILE_READ_FAILED,
            status=500,
            detail=f"Failed to start analysis: {exc}",
        )

    watcher = threading.Thread(
        target=_watch_reprocess_completion,
        args=(
            proc,
            marker_path,
            failed_path,
            segment_dir_path,
            modality,
            request_id,
        ),
        daemon=True,
    )
    watcher.start()

    data_state = _segment_data_state(segment_dir_path)
    data_state[modality] = DataState.ANALYZING.value
    return jsonify(
        {
            "data_state": data_state,
            "marker": {"started_at": marker.get("started_at", "")},
            "repair_status": "accepted",
        }
    )


@transcripts_bp.route("/api/segment/<day>/<stream>/<segment_key>", methods=["DELETE"])
def delete_segment(day: str, stream: str, segment_key: str) -> Any:
    """Delete a segment directory and all its contents.

    Removes the audio, screen recordings, transcripts and insights for one segment
    through the retention executor, which leaves a tombstone in the emptied segment
    as the owner's evidence that a deletion happened.

    Deletion is deferred: it can be cancelled until the TTL expires. Once committed
    the content is gone -- the tombstone records that it was, not what it held.

    Args:
        day: Day in YYYYMMDD format
        stream: Stream name
        segment_key: Segment directory name (HHMMSS_LEN format)

    Returns:
        JSON success response or error response
    """
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key format",
        )

    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream format")

    day_dir = str(day_path(day, create=False))
    segment_dir = str(segment_path(day, segment_key, stream, create=False))

    # Verify segment exists
    if not os.path.isdir(segment_dir):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=404,
            detail="Segment not found",
        )

    # Security check: ensure segment_dir is within day_dir
    if not os.path.commonpath([segment_dir, day_dir]) == day_dir:
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            status=403,
            detail="Invalid segment path",
        )

    try:
        ttl = SEGMENT_DELETE_TTL
        pending_id = uuid.uuid4().hex
        search_index_warning = not is_supervisor_up()

        def _commit() -> None:
            # 🔴 Every removal of the owner's media goes through the retention
            # executor. It stages the segment aside under a name no iterator
            # returns, empties it there, leaves a tombstone as the owner's evidence
            # that a deletion happened, and prunes the index BY PATH rather than
            # re-scanning the whole journal.
            #
            # ⛔ A refusal must be recorded, not swallowed. This runs on a deferred
            # thread with no caller left to raise to, so an unlogged failure is a
            # deletion the owner believes happened and did not.
            phase = "committed"
            detail: dict[str, Any] = {}
            try:
                receipt = retention_executor.remove_segments(
                    get_journal(),
                    [(day, stream, segment_key)],
                )
                detail = {
                    "removed": retention_executor.removed_paths(receipt),
                    "index": retention_executor.index_pruned(receipt),
                }
            except retention_executor.RemovalRefused as refused:
                phase = "refused"
                detail = {"refused": refused.refused.entries()}
            except retention_executor.ExecutorUnavailable as unavailable:
                phase = "failed"
                detail = {"error": str(unavailable)}

            log_app_action(
                app="transcripts",
                facet=None,
                action="segment_delete",
                params={
                    "day": day,
                    "segment_key": segment_key,
                    "stream": stream,
                    "pending_id": pending_id,
                    "phase": phase,
                    **detail,
                },
                day=day,
            )

        # See deferred_deletes module docstring: process-lifetime, pure-delete only,
        # with pending-without-terminal action-log records as a forensic signature.
        deferred_deletes.schedule_with_id(pending_id, _commit, ttl_seconds=ttl)
        log_app_action(
            app="transcripts",
            facet=None,
            action="segment_delete",
            params={
                "day": day,
                "segment_key": segment_key,
                "stream": stream,
                "pending_id": pending_id,
                "phase": "pending",
            },
            day=day,
        )

        payload = {
            "deleted": segment_key,
            "pending": pending_id,
            "commit_at_ms": int((time.time() + ttl) * 1000),
            "ttl_seconds": ttl,
        }
        if search_index_warning:
            payload["search_index_warning"] = True

        return success_response(payload)

    except Exception as e:
        return error_response(
            FILE_READ_FAILED,
            detail=f"Failed to delete segment: {e}",
        )


@transcripts_bp.route("/api/cancel-delete/<pending_id>", methods=["POST"])
def cancel_delete_segment(pending_id: str) -> Any:
    """Cancel a pending deferred segment deletion."""
    if not re.fullmatch(r"[0-9a-f]{32}", pending_id):
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="already committed or unknown",
        )

    if not deferred_deletes.cancel(pending_id):
        return error_response(
            OPERATION_NO_LONGER_AVAILABLE,
            detail="already committed or unknown",
        )

    log_app_action(
        app="transcripts",
        facet=None,
        action="segment_delete",
        params={"pending_id": pending_id, "phase": "cancelled"},
        day=datetime.now().strftime("%Y%m%d"),
    )
    return jsonify({"cancelled": pending_id})
