# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Transcript viewer app - browse and playback daily transcripts."""

from __future__ import annotations

import functools
import json
import logging
import os
import re
import shutil
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
    jsonify,
    redirect,
    render_template,
    request,
    send_file,
    url_for,
)

import solstone.think.deferred_deletes as deferred_deletes
from solstone.apps.utils import log_app_action
from solstone.convey import emit
from solstone.convey.reasons import (
    FILE_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_DAY,
    INVALID_MONTH,
    INVALID_OPERATION_FOR_STATE,
    INVALID_PATH,
    INVALID_REQUEST_VALUE,
    INVALID_SEGMENT_OR_STREAM,
    OPERATION_NO_LONGER_AVAILABLE,
    RAW_MEDIA_NOT_AVAILABLE,
)
from solstone.convey.utils import DATE_RE, error_response, format_date, success_response
from solstone.observe.hear import format_audio
from solstone.observe.screen import format_screen
from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS
from solstone.think.cluster import cluster_scan, cluster_segments, scan_day
from solstone.think.data_state import (
    DataState,
    create_analyzing_marker,
    derive_modality_state,
)
from solstone.think.entities.journal import get_journal_principal, load_journal_entity
from solstone.think.media import MIME_TYPES
from solstone.think.models import get_usage_cost
from solstone.think.pipeline_health import (
    lookup_segment_progress,
    read_segment_progress,
    segment_fully_sensed,
    segment_fully_thought,
)
from solstone.think.supervisor import is_supervisor_up
from solstone.think.utils import (
    STREAM_RE,
    day_dirs,
    day_path,
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


def _day_max_mtime(path: str) -> float:
    """Return the latest mtime under a day directory, skipping delete races."""
    day_dir = Path(path)
    try:
        max_mtime = day_dir.stat().st_mtime
    except FileNotFoundError:
        return 0.0

    try:
        for child in day_dir.rglob("*"):
            try:
                child_mtime = child.stat().st_mtime
            except FileNotFoundError:
                continue
            if child_mtime > max_mtime:
                max_mtime = child_mtime
    except FileNotFoundError:
        return max_mtime
    return max_mtime


@functools.lru_cache(maxsize=64)
def _stats_for_month(month: str, mtime_key: float) -> dict[str, int]:
    """Return cached transcript range counts for a month."""
    del mtime_key

    stats: dict[str, int] = {}
    for day_name in day_dirs().keys():
        if not day_name.startswith(month):
            continue

        audio_ranges, screen_ranges = cluster_scan(day_name)
        total_ranges = len(audio_ranges) + len(screen_ranges)
        if total_ranges > 0:
            stats[day_name] = total_ranges

    return stats


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
        if not segment_fully_sensed(seg["data_state"]):
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


@transcripts_bp.route("/")
def index() -> Any:
    """Redirect to the most recent day with segments, falling back to today."""
    today = date.today().strftime("%Y%m%d")
    for day in sorted(day_dirs().keys(), reverse=True):
        if cluster_segments(day):
            return redirect(url_for("app:transcripts.transcripts_day", day=day))
    return redirect(url_for("app:transcripts.transcripts_day", day=today))


@transcripts_bp.route("/<day>")
def transcripts_day(day: str) -> str:
    """Render transcript viewer for a specific day."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    title = format_date(day)

    return render_template("app.html", title=title)


@transcripts_bp.route("/api/ranges/<day>")
def transcript_ranges(day: str) -> Any:
    """Return available transcript ranges for a day."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    audio_ranges, screen_ranges, segments = scan_day(day)
    _attach_think_to_segments(segments, day)
    return jsonify(
        {
            "audio": _attach_streams_to_ranges(audio_ranges, segments, "audio"),
            "screen": _attach_streams_to_ranges(screen_ranges, segments, "screen"),
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
    _attach_think_to_segments(segments, day)
    return jsonify({"segments": segments})


@transcripts_bp.route("/api/day/<day>")
def transcript_day_data(day: str) -> Any:
    """Return combined ranges and segments for a day in a single response."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    audio_ranges, screen_ranges, segments = scan_day(day)
    _attach_think_to_segments(segments, day)
    return jsonify(
        {
            "audio": _attach_streams_to_ranges(audio_ranges, segments, "audio"),
            "screen": _attach_streams_to_ranges(screen_ranges, segments, "screen"),
            "segments": segments,
        }
    )


@transcripts_bp.route("/api/serve_file/<day>/<path:rel_path>")
def serve_file(day: str, rel_path: str) -> Any:
    """Serve actual media files for embedding."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, status=404, detail="Day not found")

    try:
        day_dir = day_path(day, create=False).resolve()
        full_path = (day_dir / rel_path).resolve()
        if os.path.commonpath([str(full_path), str(day_dir)]) != str(day_dir):
            return error_response(INVALID_PATH, status=403, detail="Invalid file path")
        if not full_path.is_file():
            return error_response(FILE_NOT_FOUND, detail="File not found")
    except (OSError, ValueError):
        logger.warning(
            "serve_file path validation failed for %s/%s",
            day,
            rel_path,
            exc_info=True,
        )
        return error_response(
            FILE_READ_FAILED, status=404, detail="Failed to serve file"
        )

    mimetype = MIME_TYPES.get(full_path.suffix.lower())
    if mimetype is None:
        raise ValueError(
            f"unregistered media extension for serve_file: {full_path.suffix}"
        )
    return send_file(full_path, conditional=True, mimetype=mimetype)


@transcripts_bp.route("/api/stats/<month>")
def api_stats(month: str):
    """Return transcript range counts for each day in a specific month.

    Args:
        month: YYYYMM format month string

    Returns:
        JSON dict mapping day (YYYYMMDD) to transcript range count.
        Transcripts app is not facet-aware, so returns simple {day: count} mapping.
    """
    if not MONTH_RE.fullmatch(month):
        return error_response(INVALID_MONTH, detail="Invalid month format")

    matching = [
        (day_name, path)
        for day_name, path in day_dirs().items()
        if day_name.startswith(month)
    ]
    if not matching:
        return jsonify({})

    mtime_key = max(_day_max_mtime(path) for _, path in matching)
    return jsonify(_stats_for_month(month, mtime_key))


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
    warning = False

    patterns = ("*audio.jsonl",) if modality == "audio" else ("*screen.jsonl",)
    for pattern in patterns:
        for jsonl_path in sorted(segment_dir_path.glob(pattern)):
            if not jsonl_path.is_file():
                continue
            has_jsonl = True
            try:
                entries = _load_jsonl(str(jsonl_path))
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
) -> None:
    marker_payload = _read_marker_payload(marker_path)
    payload = {
        "started_at": marker_payload.get("started_at", ""),
        "modality": marker_payload.get("modality", ""),
        "reason": reason,
        "failed_at": datetime.utcnow().isoformat(timespec="seconds") + "Z",
        "detail": detail,
    }
    tmp = failed_path.with_suffix(failed_path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(failed_path)
    marker_path.unlink(missing_ok=True)


def _watch_reprocess_completion(
    proc: subprocess.Popen,
    marker_path: Path,
    failed_path: Path,
) -> None:
    try:
        rc = proc.wait()
        stderr_tail = ""
        if proc.stderr:
            stderr_tail = (proc.stderr.read() or b"")[-512:].decode("utf-8", "replace")
        if rc == 0:
            marker_path.unlink(missing_ok=True)
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
        - segment_key: segment directory name
        - cost: processing cost in USD (float, 0.0 if no data)
        - media_sizes: dict with audio/screen byte counts for raw media files
        - media_purged: dict with audio/screen raw-reference purge flags
        - data_state: dict of advertised modality states
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
    media_sizes: dict[str, int] = {"audio": 0, "screen": 0}
    has_raw_reference = {"audio": False, "screen": False}
    has_raw_file = {"audio": False, "screen": False}
    has_raw_present = {"audio": False, "screen": False}
    has_jsonl = {"audio": False, "screen": False}
    counted_media_paths: set[Path] = set()
    warning_details: list[dict[str, str]] = []

    for raw_media in sorted(segment_dir_path.iterdir()):
        if not raw_media.is_file():
            continue
        suffix = raw_media.suffix.lower()
        if suffix in AUDIO_EXTENSIONS:
            has_raw_present["audio"] = True
            counted_media_paths.add(raw_media.resolve())
            media_sizes["audio"] += raw_media.stat().st_size
        elif suffix in VIDEO_EXTENSIONS:
            has_raw_present["screen"] = True
            counted_media_paths.add(raw_media.resolve())
            media_sizes["screen"] += raw_media.stat().st_size

    # Load speaker labels if available.
    speaker_labels_path = segment_dir_path / "talents" / "speaker_labels.json"
    speaker_map: dict[int, dict] = {}
    if speaker_labels_path.is_file():
        try:
            with open(speaker_labels_path) as f:
                labels_data = json.load(f)
            principal = get_journal_principal()
            principal_id = principal["id"] if principal else None
            entity_cache: dict[str, dict | None] = {}
            for label in labels_data.get("labels", []):
                sid = label.get("sentence_id")
                entity_id = label.get("speaker")
                confidence = label.get("confidence")
                if sid is None or not entity_id or not confidence:
                    continue
                if entity_id not in entity_cache:
                    entity_cache[entity_id] = load_journal_entity(entity_id)
                entity = entity_cache[entity_id]
                name = entity["name"] if entity else entity_id
                is_owner = entity_id == principal_id
                speaker_map[sid] = {
                    "name": name,
                    "entity_id": entity_id,
                    "confidence": confidence,
                    "is_owner": is_owner,
                }
        except (json.JSONDecodeError, OSError, KeyError):
            pass

    # Process audio files
    audio_files = glob(os.path.join(segment_dir, "*audio.jsonl"))
    for audio_path in sorted(audio_files):
        has_jsonl["audio"] = True
        try:
            entries = _load_jsonl(audio_path)
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

                chunk_sid = entry_to_sid.get(id(source))
                speaker_label = speaker_map.get(chunk_sid) if chunk_sid else None
                if speaker_label:
                    markdown = re.sub(r"Speaker \d+:\s*", "", markdown)

                chunk_data: dict[str, Any] = {
                    "type": "audio",
                    "time": time_str,
                    "timestamp": chunk.get("timestamp", 0),
                    "markdown": markdown,
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
            formatted_chunks, meta = format_screen(entries, {"file_path": screen_path})

            filename = os.path.basename(screen_path)
            monitor = (
                filename.replace("_screen.jsonl", "")
                if filename != "screen.jsonl"
                else ""
            )

            # Extract video URL from header (first entry without frame_id)
            raw_video = None
            for entry in entries:
                if "frame_id" not in entry and "raw" in entry:
                    raw_video = entry["raw"]
                    break

            # Validate raw points to a video file (skip if not, e.g. tmux)
            if raw_video and raw_video.endswith(VIDEO_EXTENSIONS):
                has_raw_reference["screen"] = True
                video_full = segment_dir_path / raw_video
                if video_full.is_file():
                    has_raw_present["screen"] = True
                    has_raw_file["screen"] = True
                    rel_path = f"{stream}/{segment_key}/{raw_video}"
                    video_files[filename] = (
                        f"/app/transcripts/api/serve_file/{day}/{rel_path}"
                    )
                    resolved = video_full.resolve()
                    if resolved not in counted_media_paths:
                        counted_media_paths.add(resolved)
                        media_sizes["screen"] += video_full.stat().st_size

            for chunk in formatted_chunks:
                source = chunk.get("source", {})
                frame_id = source.get("frame_id")
                offset = source.get("timestamp", 0)

                # Calculate wall-clock time from segment start + offset
                time_str = _format_time_from_offset(segment_key, offset)

                # Basic frames have no enriched content
                frame_content = source.get("content", {})
                is_basic = not frame_content

                # Extract participant boxes for meeting frames
                participants = []
                meeting_data = frame_content.get("meeting")
                if meeting_data:
                    for p in meeting_data.get("participants", []):
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
                        "timestamp": chunk.get("timestamp", 0),
                        "markdown": chunk.get("markdown", ""),
                        "source_ref": {
                            "frame_id": frame_id,
                            "filename": filename,
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
        # Sanctioned read-path mutation (CLAUDE.md §7 L1/L6 exception, ACs 10/11/12):
        # the shared helper may rename/unlink sidecar markers. See data_state.derive_modality_state.
        if has_chunks:
            data_state[modality] = derive_modality_state(
                segment_dir_path,
                modality,
                has_chunks=True,
                has_jsonl=has_jsonl[modality],
                has_raw=has_raw_present[modality],
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
            )
            if state != DataState.ABSENT.value:
                if modality in warning_types and state == DataState.PENDING.value:
                    state = DataState.FAILED.value
                data_state[modality] = state

    # Get cost data for this segment
    cost_data = get_usage_cost(day, segment=segment_key)

    # Collect talent .md files
    md_files = {}
    talents_dir = segment_dir_path / "talents"
    if talents_dir.is_dir():
        for md_path in sorted(talents_dir.rglob("*.md")):
            try:
                key = md_path.relative_to(talents_dir).with_suffix("").as_posix()
                md_files[key] = md_path.read_text()
            except Exception:
                continue

    # UI dedup: when a segment has structural modality data (screen/audio),
    # the structural tab already covers it — drop the matching talents/<mod>.md
    # from md_files so the tab row doesn't render two tabs labeled the same.
    # Speaker attribution reads talents/screen.md directly from disk
    # (apps/speakers/attribution.py), unaffected by this UI-side suppression.
    if "screen" in data_state:
        md_files.pop("screen", None)
    if "audio" in data_state:
        md_files.pop("audio", None)

    return jsonify(
        {
            "chunks": chunks,
            "audio_file": audio_file_url,
            "duration": audio_duration,
            "video_files": video_files,
            "md_files": md_files,
            "segment_key": segment_key,
            "cost": cost_data["cost"],
            "media_sizes": media_sizes,
            "media_purged": media_purged,
            "data_state": data_state,
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
            }
        )

    if state == DataState.FAILED.value:
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
            }
        )

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
        args=(proc, marker_path, failed_path),
        daemon=True,
    )
    watcher.start()

    data_state = _segment_data_state(segment_dir_path)
    data_state[modality] = DataState.ANALYZING.value
    marker = _read_marker_payload(marker_path)
    return jsonify(
        {
            "data_state": data_state,
            "marker": {"started_at": marker.get("started_at", "")},
        }
    )


@transcripts_bp.route("/api/segment/<day>/<stream>/<segment_key>", methods=["DELETE"])
def delete_segment(day: str, stream: str, segment_key: str) -> Any:
    """Delete a segment directory and all its contents.

    This permanently removes all audio files, screen recordings, transcripts,
    and insights for the specified segment. This action cannot be undone.

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
            shutil.rmtree(segment_dir)
            emit(
                "supervisor",
                "request",
                cmd=["sol", "indexer", "--rescan-full"],
            )
            log_app_action(
                app="transcripts",
                facet=None,
                action="segment_delete",
                params={
                    "day": day,
                    "segment_key": segment_key,
                    "stream": stream,
                    "pending_id": pending_id,
                    "phase": "committed",
                },
                day=day,
            )

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
