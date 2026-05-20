# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Gemini STT backend for speech-to-text transcription.

This module provides cloud-based speech-to-text transcription using Google's
Gemini API with speaker diarization (identifies who said what).

When VAD speech segments are provided, audio is sent as labeled clips with
explicit timestamps. Gemini returns absolute MM:SS timestamps which are then
mapped back to the audio timeline. This anchors output to known clip boundaries
rather than relying solely on Gemini's internal clock.

Enrichment (topics, setting, emotion, corrections) is handled separately by
the enrich step, same as other backends. This keeps the transcription focused
and avoids hallucinations from entity name hints in the prompt.

Environment:
- GOOGLE_API_KEY: API key (required)
"""

from __future__ import annotations

import json
import logging
import re
import time
from pathlib import Path

import numpy as np
from google.genai import types

from solstone.observe.utils import audio_to_flac_bytes
from solstone.think.models import IncompleteJSONError, generate
from solstone.think.prompts import load_prompt

logger = logging.getLogger(__name__)

_SCHEMA = json.loads(
    (Path(__file__).parent / "gemini.schema.json").read_text(encoding="utf-8")
)

# Regex for parsing speaker strings like "Speaker 1", "Speaker 2"
SPEAKER_PATTERN = re.compile(r"(?:speaker\s*)?(\d+)", re.IGNORECASE)


def _format_timestamp(seconds: float) -> str:
    """Format seconds as MM:SS timestamp string.

    Args:
        seconds: Time in seconds

    Returns:
        Formatted string like "01:23" or "12:05"
    """
    minutes = int(seconds // 60)
    secs = int(seconds % 60)
    return f"{minutes:02d}:{secs:02d}"


def _parse_timestamp(ts: str) -> float | None:
    """Parse MM:SS timestamp to seconds.

    Args:
        ts: Timestamp string like "01:23" or "1:23"

    Returns:
        Seconds as float, or None if unparseable
    """
    if not ts or not isinstance(ts, str):
        return None

    ts = ts.strip()
    if not ts:
        return None

    try:
        parts = ts.split(":")
        if len(parts) == 2:
            minutes = int(parts[0])
            seconds = float(parts[1])
            return max(0.0, minutes * 60 + seconds)
        elif len(parts) == 1:
            # Just seconds
            return max(0.0, float(parts[0]))
    except (ValueError, TypeError):
        pass

    return None


def _parse_speaker(speaker: str | int | None) -> int | None:
    """Parse speaker identifier to 1-indexed integer.

    Handles:
    - "Speaker 1" -> 1
    - "speaker 2" -> 2
    - "1" -> 1
    - 1 -> 1

    Args:
        speaker: Speaker identifier (string or int)

    Returns:
        1-indexed speaker ID, or None if unparseable
    """
    if speaker is None:
        return None

    if isinstance(speaker, int):
        return speaker if speaker > 0 else None

    if isinstance(speaker, str):
        # Try regex match for "Speaker N" pattern
        match = SPEAKER_PATTERN.search(speaker)
        if match:
            val = int(match.group(1))
            return val if val > 0 else None

        # Try direct int conversion
        try:
            val = int(speaker)
            return val if val > 0 else None
        except ValueError:
            pass

    return None


def _extract_segments(result: dict) -> list:
    """Extract the segments list from Gemini's schema-constrained response.

    Raises RuntimeError if the result does not match the documented
    {"segments": [...]} wrapper shape.
    """
    if isinstance(result, dict) and isinstance(result.get("segments"), list):
        return result["segments"]
    logger.warning(
        "Gemini returned unexpected shape: type=%s keys=%s",
        type(result).__name__,
        list(result.keys()) if isinstance(result, dict) else None,
    )
    raise RuntimeError(f"Gemini returned unexpected shape: {type(result).__name__}")


def _build_chunk_contents(
    audio: np.ndarray,
    sample_rate: int,
    speech_segments: list[tuple[float, float]],
    prompt_text: str,
) -> list:
    """Build interleaved content list with labeled audio clips.

    Creates a list of [prompt, label1, audio1, label2, audio2, ...] where
    each label tells Gemini the clip's start time and duration.

    Args:
        audio: Full audio buffer (float32, mono)
        sample_rate: Sample rate in Hz
        speech_segments: List of (start, end) tuples from VAD
        prompt_text: The transcription prompt

    Returns:
        List of content parts for Gemini API
    """
    contents: list = [prompt_text]

    for start, end in speech_segments:
        # Extract audio chunk
        start_sample = int(start * sample_rate)
        end_sample = int(end * sample_rate)
        chunk_audio = audio[start_sample:end_sample]

        # Skip empty chunks
        if len(chunk_audio) == 0:
            continue

        # Add label with start time and duration
        timestamp = _format_timestamp(start)
        duration = int(end - start)
        contents.append(f"Clip starting at {timestamp} ({duration}s):")

        # Add audio bytes
        audio_bytes = audio_to_flac_bytes(chunk_audio, sample_rate)
        contents.append(types.Part.from_bytes(data=audio_bytes, mime_type="audio/flac"))

    return contents


def _find_segment_for_timestamp(
    timestamp_seconds: float,
    speech_segments: list[tuple[float, float]],
) -> tuple[float, float]:
    """Find the VAD segment that contains or is nearest to a timestamp.

    Args:
        timestamp_seconds: Absolute timestamp in seconds
        speech_segments: List of (start, end) tuples from VAD

    Returns:
        The (start, end) tuple of the matching or nearest segment
    """
    # Check if timestamp falls within any segment
    for start, end in speech_segments:
        if start <= timestamp_seconds <= end:
            return (start, end)

    # Find nearest segment
    min_distance = float("inf")
    nearest = speech_segments[0]

    for start, end in speech_segments:
        # Distance to segment (0 if inside, otherwise distance to nearest edge)
        if timestamp_seconds < start:
            distance = start - timestamp_seconds
        else:
            distance = timestamp_seconds - end

        if distance < min_distance:
            min_distance = distance
            nearest = (start, end)

    return nearest


def _normalize_chunked_segments(
    segments: list[dict],
    speech_segments: list[tuple[float, float]],
) -> list[dict]:
    """Convert Gemini segments with MM:SS timestamps to standard statement format.

    Parses absolute timestamps from Gemini output and maps them to VAD segments.
    Falls back to segment boundaries if timestamp parsing fails.

    Args:
        segments: Raw segments from Gemini response with "start" timestamps
        speech_segments: Original VAD segments with (start, end) times

    Returns:
        List of statements with proper timestamps
    """
    statements = []
    statement_id = 1

    # Calculate overall time range for clamping
    min_time = speech_segments[0][0] if speech_segments else 0.0
    max_time = speech_segments[-1][1] if speech_segments else 0.0

    for seg in segments:
        # Get and strip text - skip if empty
        text = seg.get("text", "").strip()
        if not text:
            continue

        # Parse timestamp from Gemini output
        raw_timestamp = seg.get("start", "")
        parsed_time = _parse_timestamp(raw_timestamp)

        if parsed_time is not None:
            # Clamp to valid range
            start = max(min_time, min(parsed_time, max_time))
            # Find the segment this timestamp belongs to for the end time
            seg_start, seg_end = _find_segment_for_timestamp(start, speech_segments)
            end = seg_end
        else:
            # Fallback: use first segment boundaries
            seg_start, seg_end = speech_segments[0] if speech_segments else (0.0, 0.0)
            start = seg_start
            end = seg_end

        # Build statement
        statement = {
            "id": statement_id,
            "start": start,
            "end": end,
            "text": text,
            "words": None,  # Not available from Gemini
        }
        statement_id += 1

        # Parse speaker
        speaker = _parse_speaker(seg.get("speaker"))
        if speaker is not None:
            statement["speaker"] = speaker

        statements.append(statement)

    return statements


def _transcribe_once(
    audio: np.ndarray,
    sample_rate: int,
    config: dict,
    speech_segments: list[tuple[float, float]] | None = None,
) -> list[dict]:
    """Run one Gemini transcription request."""

    audio_duration = len(audio) / sample_rate
    use_chunks = speech_segments is not None and len(speech_segments) > 0

    if use_chunks:
        logger.info(
            f"Transcribing audio with Gemini ({audio_duration:.1f}s, "
            f"{len(speech_segments)} clips)..."
        )
    else:
        logger.info(f"Transcribing audio with Gemini ({audio_duration:.1f}s)...")

    t0 = time.perf_counter()

    # Load prompt from gemini.md
    prompt_text = load_prompt("gemini", base_dir=Path(__file__).parent).text

    # Build contents based on mode
    if use_chunks:
        contents = _build_chunk_contents(
            audio, sample_rate, speech_segments, prompt_text
        )
    else:
        # Legacy single-audio mode (for backwards compatibility)
        audio_bytes = audio_to_flac_bytes(audio, sample_rate)
        contents = [
            prompt_text,
            types.Part.from_bytes(data=audio_bytes, mime_type="audio/flac"),
        ]

    # Call Gemini via think.models.generate()
    # thinking_budget=0 disables thinking — transcription is extraction, not
    # reasoning, and Gemini's default thinking budget consumes output tokens.
    response_text = generate(
        contents=contents,
        context="observe.transcribe.gemini",
        temperature=0.3,
        max_output_tokens=16384,
        json_output=True,
        thinking_budget=0,
        json_schema=_SCHEMA,
    )

    transcribe_time = time.perf_counter() - t0
    logger.debug(
        "Gemini raw response (%d chars):\n%s", len(response_text), response_text[:2000]
    )

    # Parse JSON response
    try:
        result = json.loads(response_text)
    except json.JSONDecodeError as e:
        logger.error(f"Gemini returned invalid JSON: {e}")
        logger.debug(f"Response text: {response_text[:500]}")
        raise RuntimeError(f"Gemini returned invalid JSON: {e}") from e

    segments = _extract_segments(result)

    # Normalize to standard statement format
    if use_chunks:
        statements = _normalize_chunked_segments(segments, speech_segments)
    else:
        # Legacy mode
        statements = _normalize_chunked_segments(
            segments,
            [(0.0, audio_duration)],  # Single chunk covering entire audio
        )

    logger.info(
        f"  Gemini returned {len(statements)} segments in {transcribe_time:.2f}s"
    )

    return statements


def transcribe(
    audio: np.ndarray,
    sample_rate: int,
    config: dict,
    speech_segments: list[tuple[float, float]] | None = None,
) -> list[dict]:
    """Transcribe audio using Gemini API.

    When speech_segments is provided (from VAD), sends audio as labeled clips
    with explicit timestamps. Gemini returns absolute MM:SS timestamps which
    are mapped back to the audio timeline.

    Args:
        audio: Audio buffer (float32, mono)
        sample_rate: Sample rate in Hz (typically 16000)
        config: Backend configuration dict (currently unused)
        speech_segments: Optional list of (start, end) tuples from VAD.
            When provided, enables clip-based transcription for better
            timestamp accuracy.

    Returns:
        List of statements with id, start, end, text, speaker.
    """
    try:
        return _transcribe_once(audio, sample_rate, config, speech_segments)
    except IncompleteJSONError as original_error:
        if speech_segments is None or len(speech_segments) < 2:
            raise

        mid = len(speech_segments) // 2
        first_half = speech_segments[:mid]
        second_half = speech_segments[mid:]
        logger.info(
            "Gemini transcribe truncated at %d chunks; retrying as %d+%d split (one attempt)",
            len(speech_segments),
            len(first_half),
            len(second_half),
        )

        try:
            first_statements = _transcribe_once(audio, sample_rate, config, first_half)
            second_statements = _transcribe_once(
                audio, sample_rate, config, second_half
            )
        except Exception:
            logger.info("Gemini transcribe split-retry also truncated; raising")
            raise original_error

        statements = sorted(
            [*first_statements, *second_statements], key=lambda s: s["start"]
        )
        for i, statement in enumerate(statements):
            statement["id"] = i + 1
        logger.info(
            "Gemini transcribe split-retry succeeded; merged %d statements",
            len(statements),
        )
        return statements


def get_model_info(config: dict) -> dict:
    """Get model configuration info for metadata.

    Args:
        config: Backend configuration dict

    Returns:
        Dict with model info for JSONL metadata
    """
    # Model is resolved by think.models based on context
    # We report "gemini" as the model family
    return {
        "model": "gemini",
        "device": "cloud",
        "compute_type": "api",
    }
