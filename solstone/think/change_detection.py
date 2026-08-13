# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only segment change detection for Sense shadow mode."""

from __future__ import annotations

import hashlib
import json
import logging
from datetime import datetime
from pathlib import Path
from typing import Any

from solstone.observe.hear import load_transcript
from solstone.observe.utils import parse_screen_filename
from solstone.think.utils import DEFAULT_STREAM, iter_segments, segment_parse

# Screen-frame dHash threshold for Sense shadow-mode change detection.
SCREEN_DHASH_THRESHOLD = 8
# Bias to active: false redundant silently drops signal; false active only costs work.
TRANSCRIPT_WORD_DELTA_FLOOR = 5
# Mirrors activity_state_machine.py's current gap threshold, but remains independent.
GAP_THRESHOLD_SECONDS = 600


def assemble_sensor_state(seg_dir: Path) -> dict:
    """Assemble current per-sensor state for a segment without writing anything."""
    screen_monitors: dict[str, dict[str, str | int | None]] = {}
    for screen_path in sorted(seg_dir.glob("*screen.jsonl")):
        if not screen_path.is_file():
            continue
        position, connector = parse_screen_filename(screen_path.stem)
        header = _read_header(screen_path)
        screen_monitors[f"{position}:{connector}"] = {
            "first_hash": _normalize_hash(header.get("first_hash") if header else None),
            "last_hash": _normalize_hash(header.get("last_hash") if header else None),
            "qualified_count": _normalize_count(
                header.get("qualified_count") if header else None
            ),
        }

    transcript_text = _load_transcript_text(seg_dir)
    normalized_text = _normalize_transcript(transcript_text)
    transcript_state = {
        "present": bool(normalized_text),
        "word_count": len(normalized_text.split()) if normalized_text else 0,
        "content_hash": (
            "sha256:" + hashlib.sha256(normalized_text.encode()).hexdigest()
            if normalized_text
            else None
        ),
    }

    return {
        "screen": {"monitors": screen_monitors},
        "transcript": transcript_state,
    }


def resolve_predecessor(
    day: str, stream: str | None, segment: str
) -> dict[str, str] | None:
    """Return the chronological prior same-stream segment ref, if comparable.

    This must derive from ``iter_segments(day)`` only. Do not read
    ``last_segment_key`` or ``awareness/activity_state.json`` here: those track
    last-processed state, not the chronological predecessor, and are wrong under
    backfill or reprocess.
    """
    stream_name = stream if stream is not None else DEFAULT_STREAM
    same_stream = [
        (seg_key, seg_path)
        for seg_stream, seg_key, seg_path in iter_segments(day)
        if seg_stream == stream_name
    ]
    segment_index = next(
        (
            index
            for index, (seg_key, _seg_path) in enumerate(same_stream)
            if seg_key == segment
        ),
        None,
    )
    if segment_index is None or segment_index == 0:
        return None

    predecessor_segment, _predecessor_path = same_stream[segment_index - 1]
    predecessor_start, predecessor_end = segment_parse(predecessor_segment)
    current_start, _current_end = segment_parse(segment)
    if predecessor_start is None or predecessor_end is None or current_start is None:
        return None

    day_date = datetime.strptime(day, "%Y%m%d").date()
    predecessor_end_dt = datetime.combine(day_date, predecessor_end)
    current_start_dt = datetime.combine(day_date, current_start)
    gap_seconds = (current_start_dt - predecessor_end_dt).total_seconds()
    if gap_seconds > GAP_THRESHOLD_SECONDS:
        return None

    return {"day": day, "stream": stream_name, "segment": predecessor_segment}


def read_predecessor_state(
    day: str, predecessor_ref: dict[str, str] | None
) -> dict | None:
    """Read predecessor sensors from ``talents/change.json`` if available."""
    if predecessor_ref is None:
        return None

    target_stream = predecessor_ref["stream"]
    target_segment = predecessor_ref["segment"]
    predecessor_dir = next(
        (
            seg_path
            for seg_stream, seg_key, seg_path in iter_segments(day)
            if seg_stream == target_stream and seg_key == target_segment
        ),
        None,
    )
    if predecessor_dir is None:
        return None

    change_path = predecessor_dir / "talents" / "change.json"
    try:
        data = json.loads(change_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        logging.debug(
            "Failed to read predecessor change state %s: %s", change_path, exc
        )
        return None

    sensors = data.get("sensors") if isinstance(data, dict) else None
    if not isinstance(sensors, dict):
        return None
    if not isinstance(sensors.get("screen"), dict):
        return None
    if not isinstance(sensors.get("transcript"), dict):
        return None
    return sensors


def compare_screen(prev: dict, curr: dict) -> dict[str, bool]:
    """Compare previous screen last-hash boundaries to current first hashes."""
    prev_monitors = _screen_monitors(prev)
    curr_monitors = _screen_monitors(curr)
    present = bool(prev_monitors or curr_monitors)
    if not present:
        return {"present": False, "changed": False}

    if set(prev_monitors) != set(curr_monitors):
        return {"present": True, "changed": True}

    for monitor_key in sorted(curr_monitors):
        prev_last = _hash_to_int(prev_monitors[monitor_key].get("last_hash"))
        curr_first = _hash_to_int(curr_monitors[monitor_key].get("first_hash"))
        if prev_last is None or curr_first is None:
            return {"present": True, "changed": True}
        if (prev_last ^ curr_first).bit_count() >= SCREEN_DHASH_THRESHOLD:
            return {"present": True, "changed": True}

    return {"present": True, "changed": False}


def compare_transcript(prev: dict, curr: dict) -> dict[str, bool]:
    """Compare transcript hashes with a tight word-delta noise gate."""
    if not prev.get("present") or not curr.get("present"):
        return {"present": False, "changed": False}

    prev_hash = prev.get("content_hash")
    curr_hash = curr.get("content_hash")
    if not isinstance(prev_hash, str) or not isinstance(curr_hash, str):
        return {"present": True, "changed": True}
    if prev_hash == curr_hash:
        return {"present": True, "changed": False}

    word_delta = abs(_word_count(curr) - _word_count(prev))
    return {
        "present": True,
        "changed": word_delta > TRANSCRIPT_WORD_DELTA_FLOOR,
    }


def classify(vectors: dict) -> tuple[str, list[str]]:
    """Classify segment change state from per-sensor vectors."""
    present = any(bool(vector.get("present")) for vector in vectors.values())
    if not present:
        return "idle", []

    changed_sensors = sorted(
        name
        for name, vector in vectors.items()
        if vector.get("present") and vector.get("changed")
    )
    if changed_sensors:
        return "active", changed_sensors
    return "redundant", []


def detect_segment_change(
    day: str,
    stream: str | None,
    segment: str,
    seg_dir: Path,
    *,
    predecessor: dict | None,
    timestamp: str,
) -> dict:
    """Return the full change-detection result for persistence."""
    current_state = assemble_sensor_state(seg_dir)
    predecessor_state = read_predecessor_state(day, predecessor)

    if predecessor is None or predecessor_state is None:
        vectors = _missing_predecessor_vectors(current_state)
    else:
        vectors = {
            "screen": compare_screen(
                predecessor_state["screen"], current_state["screen"]
            ),
            "transcript": compare_transcript(
                predecessor_state["transcript"], current_state["transcript"]
            ),
        }

    change_class, changed_sensors = classify(vectors)
    return {
        "timestamp": timestamp,
        "predecessor": predecessor,
        "change_class": change_class,
        "changed_sensors": changed_sensors,
        "sensors": current_state,
    }


def _read_header(path: Path) -> dict[str, Any] | None:
    try:
        with path.open(encoding="utf-8") as handle:
            first_line = handle.readline()
        if not first_line.strip():
            return None
        header = json.loads(first_line)
    except (OSError, json.JSONDecodeError) as exc:
        logging.debug("Failed to read screen header %s: %s", path, exc)
        return None
    if not isinstance(header, dict):
        return None
    return header


def _normalize_hash(value: object) -> str | None:
    parsed = _hash_to_int(value)
    if parsed is None:
        return None
    return f"{parsed:016x}"


def _hash_to_int(value: object) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, str):
        try:
            parsed = int(value, 16)
        except ValueError:
            return None
    else:
        return None
    if not 0 <= parsed < 2**64:
        return None
    return parsed


def _normalize_count(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return max(value, 0)


def _load_transcript_text(seg_dir: Path) -> str:
    parts: list[str] = []

    jsonl_files = set()
    for pattern in ("*audio.jsonl", "*_transcript.jsonl"):
        jsonl_files.update(path for path in seg_dir.glob(pattern) if path.is_file())
    for jsonl_path in sorted(jsonl_files):
        metadata, transcript_entries, formatted_text = load_transcript(jsonl_path)
        if transcript_entries is None:
            logging.debug(
                "Skipping unreadable transcript %s: %s",
                jsonl_path,
                metadata.get("error"),
            )
            continue
        if formatted_text.strip():
            parts.append(formatted_text)

    md_files = set()
    for pattern in ("*_transcript.md", "imported.md"):
        md_files.update(path for path in seg_dir.glob(pattern) if path.is_file())
    for md_path in sorted(md_files):
        try:
            content = md_path.read_text(encoding="utf-8")
        except OSError as exc:
            logging.debug("Skipping unreadable transcript %s: %s", md_path, exc)
            continue
        if content.strip():
            parts.append(content)

    return "\n".join(parts)


def _normalize_transcript(text: str) -> str:
    return " ".join(text.lower().split()).strip()


def _screen_monitors(state: dict) -> dict:
    monitors = state.get("monitors") if isinstance(state, dict) else None
    return monitors if isinstance(monitors, dict) else {}


def _word_count(state: dict) -> int:
    value = state.get("word_count")
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return max(value, 0)


def _missing_predecessor_vectors(current_state: dict) -> dict[str, dict[str, bool]]:
    screen_present = bool(_screen_monitors(current_state["screen"]))
    transcript_present = bool(current_state["transcript"].get("present"))
    return {
        "screen": {"present": screen_present, "changed": screen_present},
        "transcript": {
            "present": transcript_present,
            "changed": transcript_present,
        },
    }
