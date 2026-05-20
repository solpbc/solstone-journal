# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the Gemini STT backend."""

from unittest.mock import Mock

import numpy as np
import pytest

from solstone.observe.transcribe.gemini import (
    _build_chunk_contents,
    _extract_segments,
    _find_segment_for_timestamp,
    _format_timestamp,
    _normalize_chunked_segments,
    _parse_speaker,
    _parse_timestamp,
    get_model_info,
    transcribe,
)
from solstone.think.models import IncompleteJSONError


class TestFormatTimestamp:
    """Tests for _format_timestamp function."""

    def test_basic_formatting(self):
        """Basic timestamp formatting."""
        assert _format_timestamp(0) == "00:00"
        assert _format_timestamp(5) == "00:05"
        assert _format_timestamp(65) == "01:05"
        assert _format_timestamp(3600) == "60:00"

    def test_minutes_and_seconds(self):
        """Minutes and seconds formatting."""
        assert _format_timestamp(90) == "01:30"
        assert _format_timestamp(125) == "02:05"
        assert _format_timestamp(599) == "09:59"


class TestParseTimestamp:
    """Tests for _parse_timestamp function."""

    def test_mm_ss_format(self):
        """MM:SS format."""
        assert _parse_timestamp("01:23") == 83.0
        assert _parse_timestamp("0:05") == 5.0
        assert _parse_timestamp("10:30") == 630.0

    def test_just_seconds(self):
        """Just seconds."""
        assert _parse_timestamp("5") == 5.0
        assert _parse_timestamp("123") == 123.0

    def test_invalid_returns_none(self):
        """Invalid timestamps return None."""
        assert _parse_timestamp("") is None
        assert _parse_timestamp(None) is None
        assert _parse_timestamp("invalid") is None

    def test_whitespace_stripped(self):
        """Whitespace is stripped."""
        assert _parse_timestamp(" 01:23 ") == 83.0


class TestParseSpeaker:
    """Tests for _parse_speaker function."""

    def test_speaker_n_format(self):
        """Speaker N format."""
        assert _parse_speaker("Speaker 1") == 1
        assert _parse_speaker("Speaker 2") == 2
        assert _parse_speaker("speaker 3") == 3  # Case insensitive

    def test_just_number(self):
        """Just a number."""
        assert _parse_speaker("1") == 1
        assert _parse_speaker("2") == 2

    def test_integer_input(self):
        """Integer input."""
        assert _parse_speaker(1) == 1
        assert _parse_speaker(2) == 2

    def test_zero_and_negative_invalid(self):
        """Zero and negative speaker IDs are invalid."""
        assert _parse_speaker(0) is None
        assert _parse_speaker(-1) is None
        assert _parse_speaker("0") is None

    def test_none_returns_none(self):
        """None input returns None."""
        assert _parse_speaker(None) is None

    def test_unparseable_returns_none(self):
        """Unparseable strings return None."""
        assert _parse_speaker("John") is None
        assert _parse_speaker("unknown") is None
        assert _parse_speaker("") is None


class TestFindSegmentForTimestamp:
    """Tests for _find_segment_for_timestamp function."""

    def test_timestamp_inside_segment(self):
        """Timestamp inside a segment returns that segment."""
        segments = [(0.0, 10.0), (15.0, 25.0), (30.0, 40.0)]
        assert _find_segment_for_timestamp(5.0, segments) == (0.0, 10.0)
        assert _find_segment_for_timestamp(20.0, segments) == (15.0, 25.0)
        assert _find_segment_for_timestamp(35.0, segments) == (30.0, 40.0)

    def test_timestamp_at_boundary(self):
        """Timestamp at segment boundary returns that segment."""
        segments = [(0.0, 10.0), (15.0, 25.0)]
        assert _find_segment_for_timestamp(0.0, segments) == (0.0, 10.0)
        assert _find_segment_for_timestamp(10.0, segments) == (0.0, 10.0)
        assert _find_segment_for_timestamp(15.0, segments) == (15.0, 25.0)

    def test_timestamp_in_gap_returns_nearest(self):
        """Timestamp in gap returns nearest segment."""
        segments = [(0.0, 10.0), (20.0, 30.0)]
        # 12 is closer to segment ending at 10 than starting at 20
        assert _find_segment_for_timestamp(12.0, segments) == (0.0, 10.0)
        # 18 is closer to segment starting at 20
        assert _find_segment_for_timestamp(18.0, segments) == (20.0, 30.0)

    def test_timestamp_before_first(self):
        """Timestamp before first segment returns first."""
        segments = [(10.0, 20.0), (30.0, 40.0)]
        assert _find_segment_for_timestamp(5.0, segments) == (10.0, 20.0)

    def test_timestamp_after_last(self):
        """Timestamp after last segment returns last."""
        segments = [(0.0, 10.0), (15.0, 25.0)]
        assert _find_segment_for_timestamp(50.0, segments) == (15.0, 25.0)


class TestNormalizeChunkedSegments:
    """Tests for _normalize_chunked_segments function."""

    def test_parses_mm_ss_timestamps(self):
        """Parses MM:SS timestamps from Gemini output."""
        segments = [
            {"start": "00:05", "speaker": "Speaker 1", "text": "Hello"},
            {"start": "00:12", "speaker": "Speaker 2", "text": "Hi there"},
        ]
        speech_segments = [(0.0, 10.0), (10.0, 20.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        assert len(statements) == 2
        assert statements[0]["start"] == 5.0
        assert statements[0]["text"] == "Hello"
        assert statements[0]["speaker"] == 1
        assert statements[1]["start"] == 12.0

    def test_clamps_timestamp_to_valid_range(self):
        """Clamps timestamps to valid range."""
        segments = [
            {"start": "10:00", "speaker": "Speaker 1", "text": "Way too late"},
        ]
        speech_segments = [(0.0, 10.0), (15.0, 25.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        # Should clamp to max_time (25.0)
        assert statements[0]["start"] == 25.0

    def test_fallback_on_invalid_timestamp(self):
        """Falls back to first segment on invalid timestamp."""
        segments = [
            {"start": "invalid", "speaker": "Speaker 1", "text": "Test"},
        ]
        speech_segments = [(5.0, 15.0), (20.0, 30.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        # Falls back to first segment start
        assert statements[0]["start"] == 5.0

    def test_assigns_end_from_containing_segment(self):
        """End time comes from the segment containing the start."""
        segments = [
            {"start": "00:22", "speaker": "Speaker 1", "text": "In second segment"},
        ]
        speech_segments = [(0.0, 10.0), (20.0, 30.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        assert statements[0]["start"] == 22.0
        assert statements[0]["end"] == 30.0  # End of containing segment

    def test_empty_text_dropped(self):
        """Segments with empty text are dropped."""
        segments = [
            {"start": "00:05", "text": "First"},
            {"start": "00:10", "text": ""},
            {"start": "00:15", "text": "   "},
            {"start": "00:20", "text": "Last"},
        ]
        speech_segments = [(0.0, 30.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        assert len(statements) == 2
        assert statements[0]["text"] == "First"
        assert statements[1]["text"] == "Last"

    def test_sequential_ids(self):
        """Statements get sequential IDs."""
        segments = [
            {"start": "00:05", "text": "First"},
            {"start": "00:10", "text": "Second"},
            {"start": "00:15", "text": "Third"},
        ]
        speech_segments = [(0.0, 30.0)]

        statements = _normalize_chunked_segments(segments, speech_segments)

        assert [s["id"] for s in statements] == [1, 2, 3]

    def test_empty_segments(self):
        """Empty segments list."""
        statements = _normalize_chunked_segments([], [(0.0, 10.0)])
        assert statements == []


class TestBuildChunkContents:
    """Tests for _build_chunk_contents function."""

    def test_basic_chunking(self):
        """Basic chunk content building."""
        audio = np.zeros(16000 * 30, dtype=np.float32)  # 30s of audio
        speech_segments = [(0.0, 10.0), (15.0, 25.0)]

        contents = _build_chunk_contents(audio, 16000, speech_segments, "Test prompt")

        # Should have: prompt + (label + audio) * 2 = 5 items
        assert len(contents) == 5
        assert contents[0] == "Test prompt"
        assert contents[1] == "Clip starting at 00:00 (10s):"
        assert contents[3] == "Clip starting at 00:15 (10s):"

    def test_duration_in_label(self):
        """Label includes duration."""
        audio = np.zeros(16000 * 30, dtype=np.float32)
        speech_segments = [(5.0, 12.0)]  # 7 second clip

        contents = _build_chunk_contents(audio, 16000, speech_segments, "Prompt")

        assert contents[1] == "Clip starting at 00:05 (7s):"

    def test_skips_empty_chunks(self):
        """Empty audio chunks are skipped."""
        audio = np.zeros(16000 * 10, dtype=np.float32)
        # Second segment has start == end (empty)
        speech_segments = [(0.0, 5.0), (5.0, 5.0), (7.0, 10.0)]

        contents = _build_chunk_contents(audio, 16000, speech_segments, "Prompt")

        # Should have: prompt + 2 valid chunks * 2 = 5 items
        assert len(contents) == 5


class TestExtractSegments:
    """Tests for _extract_segments strict wrapper parsing."""

    def test_expected_dict_wrapper(self):
        """Standard {"segments": [...]} response."""
        segs = [{"start": "00:00", "speaker": "Speaker 1", "text": "Hi"}]
        assert _extract_segments({"segments": segs}) == segs

    def test_bare_list_raises(self):
        """Bare list is rejected."""
        segs = [{"start": "00:00", "speaker": "Speaker 1", "text": "Hi"}]
        with pytest.raises(RuntimeError):
            _extract_segments(segs)

    def test_alternate_key_raises(self):
        """Alternate wrapper key is rejected."""
        segs = [{"start": "00:00", "text": "Hi"}]
        with pytest.raises(RuntimeError):
            _extract_segments({"transcript": segs})

    def test_array_wrapped_dict_raises(self):
        """Array-wrapped dict is rejected."""
        segs = [{"start": "00:00", "speaker": "Speaker 1", "text": "Hi"}]
        with pytest.raises(RuntimeError):
            _extract_segments([{"segments": segs}])

    def test_empty_segments(self):
        """Empty segments list in dict."""
        assert _extract_segments({"segments": []}) == []

    def test_empty_bare_list_raises(self):
        """Empty bare list is rejected."""
        with pytest.raises(RuntimeError):
            _extract_segments([])

    def test_non_list_segments_value_raises(self):
        """Non-list segments value is rejected."""
        with pytest.raises(RuntimeError):
            _extract_segments({"segments": "not a list"})

    def test_unexpected_type_raises(self):
        """Unexpected type is rejected."""
        with pytest.raises(RuntimeError):
            _extract_segments("unexpected")

    def test_dict_with_no_segments_key_raises(self):
        """Dict without segments key is rejected."""
        with pytest.raises(RuntimeError):
            _extract_segments({"other": 1})


class TestSplitRetry:
    def _statement(self, statement_id: int, start: float) -> dict:
        return {
            "id": statement_id,
            "start": start,
            "end": start + 1.0,
            "text": f"statement {statement_id}",
            "words": [],
            "speaker": "A",
        }

    def test_split_retry_on_truncation_returns_time_sorted_statements(
        self, monkeypatch
    ):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (3.0, 4.0)]
        original_error = IncompleteJSONError("MAX_TOKENS", "partial")
        mock_once = Mock(
            side_effect=[
                original_error,
                [self._statement(7, 10.0)],
                [self._statement(3, 2.0)],
            ]
        )
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        statements = transcribe(audio, 16000, {}, speech_segments)

        assert [s["start"] for s in statements] == [2.0, 10.0]
        assert [s["id"] for s in statements] == [1, 2]

    def test_split_retry_only_attempts_once_total_three_calls(self, monkeypatch):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0), (1.0, 2.0)]
        original_error = IncompleteJSONError("MAX_TOKENS", "partial")
        mock_once = Mock(
            side_effect=[
                original_error,
                [self._statement(1, 0.0)],
                [self._statement(2, 1.0)],
            ]
        )
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        transcribe(audio, 16000, {}, speech_segments)

        assert mock_once.call_count == 3

    def test_split_retry_half_b_failure_raises_original(self, monkeypatch):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0), (1.0, 2.0)]
        original_error = IncompleteJSONError("MAX_TOKENS", "partial")
        mock_once = Mock(
            side_effect=[
                original_error,
                [self._statement(1, 0.0)],
                RuntimeError("half b failed"),
            ]
        )
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        with pytest.raises(IncompleteJSONError) as exc_info:
            transcribe(audio, 16000, {}, speech_segments)

        assert exc_info.value is original_error
        assert mock_once.call_count == 3

    def test_split_retry_half_b_truncation_raises_original(self, monkeypatch):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0), (1.0, 2.0)]
        original_error = IncompleteJSONError("MAX_TOKENS", "partial")
        half_error = IncompleteJSONError("MAX_TOKENS", "half")
        mock_once = Mock(
            side_effect=[
                original_error,
                [self._statement(1, 0.0)],
                half_error,
            ]
        )
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        with pytest.raises(IncompleteJSONError) as exc_info:
            transcribe(audio, 16000, {}, speech_segments)

        assert exc_info.value is original_error
        assert mock_once.call_count == 3

    def test_no_split_when_fewer_than_two_chunks(self, monkeypatch):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0)]
        original_error = IncompleteJSONError("MAX_TOKENS", "partial")
        mock_once = Mock(side_effect=[original_error])
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        with pytest.raises(IncompleteJSONError) as exc_info:
            transcribe(audio, 16000, {}, speech_segments)

        assert exc_info.value is original_error
        assert mock_once.call_count == 1

    def test_happy_path_no_retry(self, monkeypatch):
        audio = np.zeros(16000, dtype=np.float32)
        speech_segments = [(0.0, 1.0), (1.0, 2.0)]
        expected = [self._statement(1, 0.0)]
        mock_once = Mock(return_value=expected)
        monkeypatch.setattr(
            "solstone.observe.transcribe.gemini._transcribe_once", mock_once
        )

        statements = transcribe(audio, 16000, {}, speech_segments)

        assert statements == expected
        assert mock_once.call_count == 1


class TestGetModelInfo:
    """Tests for get_model_info function."""

    def test_returns_expected_format(self):
        """Returns expected metadata format."""
        info = get_model_info({})

        assert info["model"] == "gemini"
        assert info["device"] == "cloud"
        assert info["compute_type"] == "api"


class TestBackendRegistry:
    """Tests for backend registry integration."""

    def test_gemini_registered(self):
        """Gemini backend is registered."""
        from solstone.observe.transcribe import BACKEND_REGISTRY

        assert "gemini" in BACKEND_REGISTRY

    def test_get_backend(self):
        """Can get Gemini backend module."""
        from solstone.observe.transcribe import get_backend

        backend = get_backend("gemini")
        assert hasattr(backend, "transcribe")
        assert hasattr(backend, "get_model_info")
