# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard the canonical analyzed reference day against stub regressions.

The fixture day ``20260304`` is the transcripts dashboard visual-review
reference day. This test keeps it from drifting into smoke, stub, or
unanalyzed content. See
``tests/fixtures/journal/chronicle/20260304/README.md`` for the prose
invariant.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from solstone.think.utils import get_journal

REFERENCE_DAY = "20260304"
EXPECTED_SEGMENTS = {"090000_300", "140000_300", "180000_300"}
MIN_MEDIA_BYTES = 100
MEDIA_SUFFIXES = {".png", ".webm", ".flac", ".wav", ".mp4", ".jpg", ".jpeg"}


def _reference_day_dir() -> Path:
    return Path(get_journal()) / "chronicle" / REFERENCE_DAY


def _default_dir() -> Path:
    return _reference_day_dir() / "default"


def _segment_dirs() -> list[Path]:
    default_dir = _default_dir()
    return sorted(
        (path for path in default_dir.glob("*") if path.is_dir()),
        key=lambda path: path.name,
    )


def _jsonl_objects(path: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as handle:
        for line_number, raw_line in enumerate(handle, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as exc:
                raise AssertionError(f"{path}:{line_number} is invalid JSON") from exc
            assert isinstance(value, dict), (
                f"{path}:{line_number} must be a JSON object, got "
                f"{type(value).__name__}"
            )
            entries.append(value)
    return entries


def _json_value(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise AssertionError(f"{path} is invalid JSON") from exc


def _zero_cost_paths(value: Any, location: str = "$") -> list[str]:
    if isinstance(value, dict):
        matches: list[str] = []
        cost = value.get("cost")
        if (
            isinstance(cost, (int, float))
            and not isinstance(cost, bool)
            and cost == 0.0
        ):
            matches.append(f"{location}.cost")
        for key, child in value.items():
            matches.extend(_zero_cost_paths(child, f"{location}.{key}"))
        return matches
    if isinstance(value, list):
        matches = []
        for index, child in enumerate(value):
            matches.extend(_zero_cost_paths(child, f"{location}[{index}]"))
        return matches
    return []


def test_reference_day_has_exact_segment_set() -> None:
    discovered = {path.name for path in _segment_dirs()}
    missing = EXPECTED_SEGMENTS - discovered
    extra = discovered - EXPECTED_SEGMENTS

    assert discovered == EXPECTED_SEGMENTS, (
        f"{REFERENCE_DAY} segment set changed; "
        f"missing={sorted(missing)} extra={sorted(extra)}"
    )


@pytest.mark.parametrize("segment", sorted(EXPECTED_SEGMENTS))
def test_each_segment_screen_is_analyzed(segment: str) -> None:
    screen_path = _default_dir() / segment / "screen.jsonl"
    assert screen_path.is_file(), f"{segment}: missing {screen_path}"
    entries = _jsonl_objects(screen_path)

    assert any("analysis" in entry for entry in entries[1:]), (
        f"{segment}: {screen_path} has no analyzed screen entries"
    )


@pytest.mark.parametrize("segment", sorted(EXPECTED_SEGMENTS))
def test_each_segment_audio_has_statements(segment: str) -> None:
    audio_path = _default_dir() / segment / "audio.jsonl"
    assert audio_path.is_file(), f"{segment}: missing {audio_path}"
    entries = _jsonl_objects(audio_path)

    assert entries, f"{segment}: {audio_path} is empty"
    header = entries[0]
    missing_header_keys = {"raw", "model", "duration"} - set(header)
    assert not missing_header_keys, (
        f"{segment}: {audio_path} header missing keys {sorted(missing_header_keys)}"
    )
    assert any(
        isinstance(entry.get("text"), str) and entry["text"].strip()
        for entry in entries[1:]
    ), f"{segment}: {audio_path} has no non-empty audio statements"


@pytest.mark.parametrize("segment", sorted(EXPECTED_SEGMENTS))
def test_each_segment_monitor_diff_has_content(segment: str) -> None:
    monitor_path = _default_dir() / segment / "monitor_1_diff.json"
    assert monitor_path.is_file(), f"{segment}: missing {monitor_path}"
    data = _json_value(monitor_path)
    assert isinstance(data, dict), f"{segment}: {monitor_path} must be a JSON object"

    visual_description = data.get("visual_description")
    full_ocr = data.get("full_ocr")
    has_visual_description = isinstance(visual_description, str) and bool(
        visual_description.strip()
    )
    has_full_ocr = isinstance(full_ocr, str) and bool(full_ocr.strip())

    assert has_visual_description or has_full_ocr, (
        f"{segment}: {monitor_path} lacks visual_description and full_ocr content"
    )


def test_no_placeholder_media_files() -> None:
    for path in _reference_day_dir().rglob("*"):
        if not path.is_file() or path.suffix.lower() not in MEDIA_SUFFIXES:
            continue
        size = path.stat().st_size
        assert size > MIN_MEDIA_BYTES, (
            f"{path} is placeholder-sized media: {size} bytes <= {MIN_MEDIA_BYTES}"
        )


def test_no_zero_cost_smoke_marker() -> None:
    for path in _reference_day_dir().rglob("*"):
        if not path.is_file() or path.suffix.lower() not in {".json", ".jsonl"}:
            continue
        if path.suffix.lower() == ".jsonl":
            for line_number, value in enumerate(_jsonl_objects(path), start=1):
                matches = _zero_cost_paths(value)
                assert not matches, (
                    f"{path}:{line_number} contains zero-cost smoke marker "
                    f"at {matches[0]}"
                )
            continue

        matches = _zero_cost_paths(_json_value(path))
        assert not matches, f"{path} contains zero-cost smoke marker at {matches[0]}"
