# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Transcript segmentation utilities using LLM analysis."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import List, Optional

from .prompts import load_prompt

_SEGMENT_SCHEMA = json.loads(
    (Path(__file__).parent / "detect_transcript_segment.schema.json").read_text(
        encoding="utf-8"
    )
)
# Source of truth is think/detect_transcript_json.md.
_JSON_SCHEMA = json.loads(
    (Path(__file__).parent / "detect_transcript_json.schema.json").read_text(
        encoding="utf-8"
    )
)


def _load_json_prompt() -> str:
    """Load the JSON system prompt."""
    return load_prompt("detect_transcript_json", base_dir=Path(__file__).parent).text


def _load_segment_prompt() -> str:
    """Load the system prompt for segment detection."""
    return load_prompt("detect_transcript_segment", base_dir=Path(__file__).parent).text


def number_lines(text: str) -> tuple[str, List[str]]:
    """Return text with prefixed line numbers and the original lines."""
    lines = text.splitlines()
    numbered = "\n".join(f"{idx + 1}: {line}" for idx, line in enumerate(lines))
    return numbered, lines


def parse_segment_boundaries(json_text: str, num_lines: int) -> List[dict]:
    """Validate and return segment boundaries from ``json_text``.

    Args:
        json_text: JSON array of {"start_at": "HH:MM:SS", "line": N} objects
        num_lines: Total number of lines in the transcript

    Returns:
        List of boundary dicts with "start_at" and "line" keys
    """
    try:
        data = json.loads(json_text)
    except json.JSONDecodeError as exc:  # pragma: no cover - network errors
        logging.error("Failed to parse JSON response")
        raise ValueError("invalid JSON") from exc

    if not isinstance(data, list) or not data:
        logging.error("JSON response is not a non-empty list")
        raise ValueError("expected non-empty list")

    boundaries: List[dict] = []
    last_line = 0
    for item in data:
        if not isinstance(item, dict):
            logging.error(f"Invalid boundary type: {type(item)}")
            raise ValueError("boundaries must be objects")

        if "start_at" not in item or "line" not in item:
            logging.error(f"Missing required fields in boundary: {item}")
            raise ValueError("boundary must have 'start_at' and 'line' fields")

        line = item["line"]
        start_at = item["start_at"]

        if (
            not isinstance(line, int)
            or line <= last_line
            or line < 1
            or line > num_lines
        ):
            logging.error(
                f"Invalid line number: {line} (last: {last_line}, max: {num_lines})"
            )
            raise ValueError("invalid line number")

        if not isinstance(start_at, str):
            logging.error(f"Invalid start_at type: {type(start_at)}")
            raise ValueError("start_at must be a string")

        boundaries.append({"start_at": start_at, "line": line})
        last_line = line

    logging.info(f"Successfully parsed {len(boundaries)} segment boundaries")
    return boundaries


def segments_from_boundaries(
    lines: List[str], boundaries: List[dict]
) -> List[tuple[str, str]]:
    """Return transcript segments split at boundaries.

    Args:
        lines: Original transcript lines
        boundaries: List of {"start_at": "HH:MM:SS", "line": N} dicts

    Returns:
        List of (start_at, text) tuples for each segment
    """
    segments: List[tuple[str, str]] = []

    for idx, boundary in enumerate(boundaries):
        start_at = boundary["start_at"]
        start_line = boundary["line"]

        # Determine end line (next boundary or end of file)
        if idx + 1 < len(boundaries):
            end_line = boundaries[idx + 1]["line"]
            segment_lines = lines[start_line - 1 : end_line - 1]  # noqa: E203
        else:
            segment_lines = lines[start_line - 1 :]  # noqa: E203

        text = "\n".join(segment_lines).strip()
        segments.append((start_at, text))

    logging.info(f"Created {len(segments)} transcript segments")
    return segments


def detect_transcript_segment(text: str, start_time: str) -> List[tuple[str, str]]:
    """Return transcript segments with absolute timestamps using LLM analysis.

    Args:
        text: The transcript text to segment
        start_time: Absolute start time in HH:MM:SS format

    Returns:
        List of (start_at, text) tuples where start_at is absolute HH:MM:SS.
        Returns empty list on LLM or parsing failure.
    """
    numbered, lines = number_lines(text)
    # Prepend START_TIME for the prompt
    contents = f"START_TIME: {start_time}\n{numbered}"
    logging.info(f"Starting transcript segmentation (start: {start_time})...")

    from solstone.think.models import generate

    try:
        response_text = generate(
            contents=contents,
            context="observe.detect.segment",
            temperature=0.3,
            max_output_tokens=4096,
            thinking_budget=8192,
            system_instruction=_load_segment_prompt(),
            json_output=True,
            json_schema=_SEGMENT_SCHEMA,
        )

        logging.info(f"Received segmentation response: {response_text}")
        boundaries = parse_segment_boundaries(response_text, len(lines))
        segments = segments_from_boundaries(lines, boundaries)

        return segments
    except (ValueError, json.JSONDecodeError) as e:
        logging.error(f"Transcript segmentation failed: {e}")
        return []


def detect_transcript_json(text: str, segment_start: str) -> Optional[dict]:
    """Return transcript ``text`` converted to JSON using LLM analysis.

    Args:
        text: The transcript segment text
        segment_start: Absolute start time of this segment in HH:MM:SS format

    Returns:
        Wrapper dict with ``entries``, ``topics``, and ``setting`` keys
    """
    logging.info(
        f"Starting transcript JSON conversion (segment_start: {segment_start})..."
    )

    # Prepend SEGMENT_START for the prompt
    contents = f"SEGMENT_START: {segment_start}\n{text}"

    from solstone.think.models import generate

    response_text = generate(
        contents=contents,
        context="observe.detect.json",
        temperature=0.3,
        max_output_tokens=8192,
        thinking_budget=8192,
        system_instruction=_load_json_prompt(),
        json_output=True,
        json_schema=_JSON_SCHEMA,
    )

    logging.info(f"Received JSON conversion response: {response_text[:100]}")
    try:
        result = json.loads(response_text)
        logging.info("Successfully converted transcript to JSON")
        return result
    except json.JSONDecodeError:
        logging.error("Failed to parse JSON response from LLM")
        return None


__all__ = [
    "detect_transcript_segment",
    "detect_transcript_json",
    "number_lines",
    "parse_segment_boundaries",
    "segments_from_boundaries",
]
