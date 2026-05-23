# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
import os
import shutil
import time
from pathlib import Path

import pytest

from solstone.think.utils import day_path


def test_cluster(tmp_path, monkeypatch):
    """Test cluster() uses transcripts and agent output summaries (*.md files)."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")
    # Write JSONL format: metadata first, then entry in segment directory
    (day_dir / "default" / "120000_300").mkdir(parents=True)
    (day_dir / "default" / "120000_300" / "audio.jsonl").write_text(
        '{}\n{"text": "hi"}\n'
    )
    (day_dir / "default" / "120500_300").mkdir(parents=True)
    (day_dir / "default" / "120500_300" / "talents").mkdir()
    (day_dir / "default" / "120500_300" / "talents" / "screen.md").write_text(
        "screen summary"
    )
    result, counts = mod.cluster(
        "20240101", sources={"transcripts": True, "percepts": False, "agents": True}
    )
    assert counts["transcripts"] == 1
    assert counts["agents"] == 1
    assert "### Transcript" in result
    # Now uses insight rendering: "### {stem} summary"
    assert "screen summary" in result


def test_cluster_range(tmp_path, monkeypatch):
    """Test cluster_range with transcripts and agents sources."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")
    # Write JSONL format: metadata first, then entry with proper start time and source in segment directory
    (day_dir / "default" / "120000_300").mkdir(parents=True)
    (day_dir / "default" / "120000_300" / "audio.jsonl").write_text(
        '{"raw": "raw.flac", "model": "whisper-1"}\n'
        '{"start": "00:00:01", "source": "mic", "text": "hi from audio"}\n'
    )
    (day_dir / "default" / "120000_300" / "talents").mkdir()
    (day_dir / "default" / "120000_300" / "talents" / "screen.md").write_text(
        "screen summary content"
    )
    # Test with agents=True to include *.md files
    md = mod.cluster_range(
        "20240101",
        "120000",
        "120100",
        sources={"transcripts": True, "percepts": False, "agents": True},
    )
    # Check that the function works and includes expected sections
    assert "### Transcript" in md
    # Now uses insight rendering: "### {stem} summary"
    assert "screen summary" in md
    assert "screen summary content" in md


def test_cluster_scan(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")
    # Audio transcripts at 09:01, 09:05, 09:20 and 11:00 (JSONL format with empty metadata)
    (day_dir / "default" / "090101_300").mkdir(parents=True)
    (day_dir / "default" / "090101_300" / "audio.jsonl").write_text("{}\n")
    (day_dir / "default" / "090500_300").mkdir(parents=True)
    (day_dir / "default" / "090500_300" / "audio.jsonl").write_text("{}\n")
    (day_dir / "default" / "092000_300").mkdir(parents=True)
    (day_dir / "default" / "092000_300" / "audio.jsonl").write_text("{}\n")
    (day_dir / "default" / "110000_300").mkdir(parents=True)
    (day_dir / "default" / "110000_300" / "audio.jsonl").write_text("{}\n")
    # Screen transcripts at 10:01, 10:05, 10:20 and 12:00
    (day_dir / "default" / "100101_300").mkdir(parents=True)
    (day_dir / "default" / "100101_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )
    (day_dir / "default" / "100500_300").mkdir(parents=True)
    (day_dir / "default" / "100500_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )
    (day_dir / "default" / "102000_300").mkdir(parents=True)
    (day_dir / "default" / "102000_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )
    (day_dir / "default" / "120000_300").mkdir(parents=True)
    (day_dir / "default" / "120000_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )
    audio_ranges, screen_ranges = mod.cluster_scan("20240101")
    # Expected ranges: 15-minute slot grouping (segments 09:01-09:05-09:20 group together)
    # Slots: 09:00, 09:00, 09:15 -> ranges: 09:00-09:30; 11:00 -> 11:00-11:15
    assert audio_ranges == [("09:00", "09:30"), ("11:00", "11:15")]
    assert screen_ranges == [("10:00", "10:30"), ("12:00", "12:15")]


def test_cluster_segments(tmp_path, monkeypatch):
    """Test cluster_segments returns individual segments with their types."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with duration: 090000_300 (09:00:00 for 5 minutes)
    (day_dir / "default" / "090000_300").mkdir(parents=True)
    (day_dir / "default" / "090000_300" / "audio.jsonl").write_text("{}\n")

    # Create segment with both audio and screen
    (day_dir / "default" / "100000_600").mkdir(parents=True)
    (day_dir / "default" / "100000_600" / "audio.jsonl").write_text("{}\n")
    (day_dir / "default" / "100000_600" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )

    # Create segment with only screen
    (day_dir / "default" / "110000_300").mkdir(parents=True)
    (day_dir / "default" / "110000_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
    )

    segments = mod.cluster_segments("20240101")

    assert len(segments) == 3

    # Check first segment (audio only)
    assert segments[0]["key"] == "090000_300"
    assert segments[0]["start"] == "09:00"
    assert segments[0]["end"] == "09:05"
    assert segments[0]["types"] == ["audio"]

    # Check second segment (both transcripts and screen)
    assert segments[1]["key"] == "100000_600"
    assert segments[1]["start"] == "10:00"
    assert segments[1]["end"] == "10:10"
    assert "audio" in segments[1]["types"]
    assert "screen" in segments[1]["types"]

    # Check third segment (screen only)
    assert segments[2]["key"] == "110000_300"
    assert segments[2]["start"] == "11:00"
    assert segments[2]["end"] == "11:05"
    assert segments[2]["types"] == ["screen"]


def test_cluster_period_uses_raw_screen(tmp_path, monkeypatch):
    """Test cluster_period uses raw screen.jsonl, not insight *.md files."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with both audio and raw screen data
    segment = day_dir / "default" / "100000_300"
    segment.mkdir(parents=True)
    (segment / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "hello"}\n'
    )
    # Raw screen.jsonl with frame analysis (what cluster_period should use)
    (segment / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "code_editor", '
        '"visual_description": "VS Code with Python file"}}\n'
    )
    # Also create screen.md (insight) to verify it's NOT used by cluster_period
    (segment / "talents").mkdir()
    (segment / "talents" / "screen.md").write_text("This insight should NOT appear")

    result, counts = mod.cluster_period(
        "20240101",
        "100000_300",
        sources={"transcripts": True, "percepts": True, "agents": False},
    )

    # Should have both transcript and screen entries
    assert counts["transcripts"] == 1
    assert counts["percepts"] == 1
    assert "### Transcript" in result
    # Should use raw screen format header
    assert "Screen Activity" in result
    # Raw screen content should be present
    assert "VS Code with Python file" in result
    # Insight content should NOT be present (agents=False for cluster_period)
    assert "This insight should NOT appear" not in result


def test_load_entries_from_toplevel_segment(tmp_path, monkeypatch):
    """_load_entries_from_segment resolves the day for top-level segment dirs."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")
    segment = day_dir / "100000_300"
    segment.mkdir()

    mod = importlib.import_module("solstone.think.cluster")

    entries = mod._load_entries_from_segment(
        str(segment),
        transcripts=True,
        percepts=False,
        agents=False,
    )

    assert entries == []


def test_cluster_range_with_agents(tmp_path, monkeypatch):
    """Test cluster_range with agents source loads all *.md files."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with multiple insight files
    segment = day_dir / "default" / "100000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "hello"}\n'
    )
    (segment / "talents" / "screen.md").write_text("Screen activity summary")
    (segment / "talents" / "activity.md").write_text("Activity insight content")
    # Also create screen.jsonl to verify it's NOT used when agents=True, screen=False
    (segment / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "code_editor"}}\n'
    )

    # Test agents=True returns *.md summaries, not raw screen data
    result = mod.cluster_range(
        "20240101",
        "100000",
        "100500",
        sources={"transcripts": True, "percepts": False, "agents": True},
    )

    assert "### Transcript" in result
    # Should include both .md files as agent outputs
    assert "### screen summary" in result
    assert "Screen activity summary" in result
    assert "### activity summary" in result
    assert "Activity insight content" in result
    # Should NOT include raw screen data
    assert "code_editor" not in result


def test_cluster_range_with_screen(tmp_path, monkeypatch):
    """Test cluster_range with screen source loads raw screen.jsonl data."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with raw screen data and insight file
    segment = day_dir / "default" / "100000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "code_editor"}}\n'
    )
    (segment / "talents" / "screen.md").write_text("Screen summary insight")

    # Test screen=True returns raw screen data, not agent outputs
    result = mod.cluster_range(
        "20240101",
        "100000",
        "100500",
        sources={"transcripts": False, "percepts": True, "agents": False},
    )

    assert "Screen Activity" in result
    assert "code_editor" in result
    # Should NOT include insight content
    assert "Screen summary insight" not in result
    assert "### screen summary" not in result


def test_cluster_range_with_multiple_screen_files(tmp_path, monkeypatch):
    """Test cluster_range loads multiple *_screen.jsonl files per segment."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with multiple screen files (like multi-monitor setup)
    segment = day_dir / "default" / "100000_300"
    segment.mkdir(parents=True)
    (segment / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "code_editor", '
        '"visual_description": "Primary monitor with VS Code"}}\n'
    )
    (segment / "monitor_2_screen.jsonl").write_text(
        '{"raw": "monitor_2.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "browser", '
        '"visual_description": "Secondary monitor with documentation"}}\n'
    )

    # Test screen=True returns data from both screen files
    result = mod.cluster_range(
        "20240101",
        "100000",
        "100500",
        sources={"transcripts": False, "percepts": True, "agents": False},
    )

    # Should include content from both screen files
    assert "Primary monitor with VS Code" in result
    assert "Secondary monitor with documentation" in result


def test_cluster_scan_with_split_screen(tmp_path, monkeypatch):
    """Test cluster_scan detects *_screen.jsonl files."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with only *_screen.jsonl (no screen.jsonl)
    (day_dir / "default" / "100000_300").mkdir(parents=True)
    (day_dir / "default" / "100000_300" / "monitor_1_screen.jsonl").write_text(
        '{"raw": "m1.webm"}\n'
    )

    audio_ranges, screen_ranges = mod.cluster_scan("20240101")

    # Should detect the segment as having screen content (15-minute slot grouping)
    assert screen_ranges == [("10:00", "10:15")]


def test_cluster_segments_with_split_screen(tmp_path, monkeypatch):
    """Test cluster_segments detects *_screen.jsonl files."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with only *_screen.jsonl (no screen.jsonl)
    (day_dir / "default" / "100000_300").mkdir(parents=True)
    (day_dir / "default" / "100000_300" / "wayland_screen.jsonl").write_text(
        '{"raw": "w.webm"}\n'
    )

    segments = mod.cluster_segments("20240101")

    assert len(segments) == 1
    assert segments[0]["key"] == "100000_300"
    assert "screen" in segments[0]["types"]


def test_cluster_span(tmp_path, monkeypatch):
    """Test cluster_span processes a span of segments."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create three segments with different content
    (day_dir / "default" / "090000_300").mkdir(parents=True)
    (day_dir / "default" / "090000_300" / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "morning segment"}\n'
    )

    (day_dir / "default" / "100000_300").mkdir(parents=True)
    (day_dir / "default" / "100000_300" / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "mid-morning segment"}\n'
    )
    (day_dir / "default" / "100000_300" / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n'
        '{"timestamp": 10, "analysis": {"primary": "code_editor"}}\n'
    )

    (day_dir / "default" / "110000_300").mkdir(parents=True)
    (day_dir / "default" / "110000_300" / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "late morning segment"}\n'
    )

    # Process only first and third segments as a span (audio only, no screen)
    result, counts = mod.cluster_span(
        "20240101",
        ["090000_300", "110000_300"],
        sources={"transcripts": True, "percepts": False, "agents": False},
    )

    # Should have 2 transcript entries (one per segment)
    assert counts["transcripts"] == 2
    assert counts["percepts"] == 0
    assert "morning segment" in result
    assert "late morning segment" in result
    # Should NOT include the skipped segment
    assert "mid-morning segment" not in result
    assert "code_editor" not in result


def test_cluster_span_missing_segment(tmp_path, monkeypatch):
    """Test cluster_span fails fast when segment is missing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create only one segment
    (day_dir / "default" / "090000_300").mkdir(parents=True)
    (day_dir / "default" / "090000_300" / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n'
    )

    # Try to process existing and non-existing segments
    with pytest.raises(ValueError) as exc_info:
        mod.cluster_span(
            "20240101",
            ["090000_300", "100000_300"],
            sources={"transcripts": True, "percepts": False, "agents": False},
        )

    assert "100000_300" in str(exc_info.value)
    assert "not found" in str(exc_info.value)


def test_cluster_with_agent_filter_dict(tmp_path, monkeypatch):
    """Test cluster() with dict-valued agents source for selective filtering."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with multiple agent output files
    segment = day_dir / "default" / "120000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "audio.jsonl").write_text('{}\n{"text": "hello"}\n')
    (segment / "talents" / "entities.md").write_text("Entity extraction results")
    (segment / "talents" / "meetings.md").write_text("Meeting summary results")
    (segment / "talents" / "flow.md").write_text("Flow analysis results")

    # Test filtering to only include entities
    result, counts = mod.cluster(
        "20240101",
        sources={"transcripts": True, "percepts": False, "agents": {"entities": True}},
    )

    assert counts["transcripts"] == 1
    assert counts["agents"] == 1  # Only entities should be counted
    assert "Entity extraction results" in result
    assert "Meeting summary results" not in result
    assert "Flow analysis results" not in result


def test_cluster_with_agent_filter_multiple(tmp_path, monkeypatch):
    """Test cluster() with dict selecting multiple agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with multiple agent output files
    segment = day_dir / "default" / "120000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "audio.jsonl").write_text('{}\n{"text": "hello"}\n')
    (segment / "talents" / "entities.md").write_text("Entity extraction results")
    (segment / "talents" / "meetings.md").write_text("Meeting summary results")
    (segment / "talents" / "flow.md").write_text("Flow analysis results")

    # Test filtering to include entities and meetings but not flow
    result, counts = mod.cluster(
        "20240101",
        sources={
            "transcripts": True,
            "percepts": False,
            "agents": {"entities": True, "meetings": "required", "flow": False},
        },
    )

    assert counts["transcripts"] == 1
    assert counts["agents"] == 2  # entities + meetings
    assert "Entity extraction results" in result
    assert "Meeting summary results" in result
    assert "Flow analysis results" not in result


def test_cluster_with_agent_filter_app_namespaced(tmp_path, monkeypatch):
    """Test cluster() with dict filtering app-namespaced agent outputs."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    # Create segment with app-namespaced agent output files
    # App agent output naming: "app:agent" -> "_app_agent.md"
    segment = day_dir / "default" / "120000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "audio.jsonl").write_text('{}\n{"text": "hello"}\n')
    (segment / "talents" / "entities.md").write_text("System entity results")
    (segment / "talents" / "_todos_review.md").write_text("Todos review results")

    # Test filtering to include app-namespaced agent
    result, counts = mod.cluster(
        "20240101",
        sources={
            "transcripts": True,
            "percepts": False,
            "agents": {"entities": False, "todos:review": True},
        },
    )

    assert counts["transcripts"] == 1
    assert counts["agents"] == 1  # Only todos:review
    assert "System entity results" not in result
    assert "Todos review results" in result


def test_cluster_with_empty_agent_filter(tmp_path, monkeypatch):
    """Test cluster() with empty dict means no agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "120000_300"
    segment.mkdir(parents=True)
    (segment / "talents").mkdir()
    (segment / "audio.jsonl").write_text('{}\n{"text": "hello"}\n')
    (segment / "talents" / "entities.md").write_text("Entity extraction results")

    # Empty dict should mean no agents
    result, counts = mod.cluster(
        "20240101",
        sources={"transcripts": True, "percepts": False, "agents": {}},
    )

    assert counts["transcripts"] == 1
    assert counts["agents"] == 0
    assert "Entity extraction results" not in result


def test_filename_to_agent_key():
    """Test _filename_to_agent_key conversion."""
    from solstone.think.cluster import _filename_to_agent_key

    # System agents
    assert _filename_to_agent_key("entities") == "entities"
    assert _filename_to_agent_key("flow") == "flow"

    # App-namespaced agents
    assert _filename_to_agent_key("_todos_review") == "todos:review"
    assert _filename_to_agent_key("_entities_observer") == "entities:observer"

    # Edge case: single underscore component
    assert _filename_to_agent_key("_app") == "_app"  # No second part, returns as-is


def test_agent_matches_filter():
    """Test _agent_matches_filter logic."""
    from solstone.think.cluster import _agent_matches_filter

    # None filter means all agents
    assert _agent_matches_filter("entities", None) is True
    assert _agent_matches_filter("_todos_review", None) is True

    # Empty dict means no agents
    assert _agent_matches_filter("entities", {}) is False
    assert _agent_matches_filter("_todos_review", {}) is False

    # Specific filtering
    filter_dict = {"entities": True, "meetings": False, "todos:review": "required"}
    assert _agent_matches_filter("entities", filter_dict) is True
    assert _agent_matches_filter("meetings", filter_dict) is False
    assert _agent_matches_filter("_todos_review", filter_dict) is True
    assert _agent_matches_filter("flow", filter_dict) is False  # Not in filter


def test_scan_day_combined(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    first = day_dir / "default" / "090000_300"
    first.mkdir(parents=True)
    (first / "audio.jsonl").write_text("{}\n")
    (first / "screen.jsonl").write_text('{"raw": "screen.webm"}\n')

    second = day_dir / "default" / "093000_300"
    second.mkdir(parents=True)
    (second / "audio.jsonl").write_text("{}\n")

    audio_ranges, screen_ranges, segments = mod.scan_day("20240101")
    expected_ranges = mod.cluster_scan("20240101")
    expected_segments = mod.cluster_segments("20240101")

    assert audio_ranges == [("09:00", "09:15"), ("09:30", "09:45")]
    assert screen_ranges == [("09:00", "09:15")]
    assert segments == [
        {
            "key": "090000_300",
            "start": "09:00",
            "end": "09:05",
            "types": ["audio", "screen"],
            "stream": "default",
            "data_state": {"audio": "pending", "screen": "pending"},
        },
        {
            "key": "093000_300",
            "start": "09:30",
            "end": "09:35",
            "types": ["audio"],
            "stream": "default",
            "data_state": {"audio": "pending"},
        },
    ]
    assert (audio_ranges, screen_ranges) == expected_ranges
    assert segments == expected_segments


def test_scan_day_empty(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    mod = importlib.import_module("solstone.think.cluster")

    assert mod.scan_day("20250101") == ([], [], [])


def test_scan_day_marks_stub_screen_pending(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "screen.jsonl").write_text('{"raw": "screen.webm"}\n')

    audio_ranges, screen_ranges, segments = mod.scan_day("20240101")

    assert audio_ranges == []
    assert screen_ranges == [("09:00", "09:15")]
    assert segments == [
        {
            "key": "090000_300",
            "start": "09:00",
            "end": "09:05",
            "types": ["screen"],
            "stream": "default",
            "data_state": {"screen": "pending"},
        }
    ]


def test_scan_day_marks_headerless_screen_frame_analyzed(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    frame = {
        "frame_id": 1,
        "timestamp": 1,
        "analysis": {
            "primary": "work",
            "visual_description": "fedora tmux session",
        },
        "content": {},
    }
    (segment / "fedora_tmux_screen.jsonl").write_text(json.dumps(frame) + "\n")

    _, screen_ranges, segments = mod.scan_day("20240101")

    assert screen_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"screen": "analyzed"}


def test_scan_day_marks_analyzed_screen_analyzed(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "screen.jsonl").write_text(
        '{"raw": "screen.webm"}\n{"timestamp": 1, "analysis": {"primary": "work"}}\n'
    )

    _, screen_ranges, segments = mod.scan_day("20240101")

    assert screen_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"screen": "analyzed"}


def test_scan_day_keeps_screen_raw_substring_collision_pending(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "screen.jsonl").write_text('{"raw": "clip_timestamp.webm"}\n')

    _, screen_ranges, segments = mod.scan_day("20240101")

    assert screen_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"screen": "pending"}


def test_scan_day_marks_whitespace_only_screen_pending(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "screen.jsonl").write_text("\n  \n\t\n")

    _, screen_ranges, segments = mod.scan_day("20240101")

    assert screen_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"screen": "pending"}


@pytest.mark.parametrize("raw_name", ["audio.flac", "audio.m4a"])
def test_scan_day_marks_raw_audio_without_jsonl_pending(
    tmp_path, monkeypatch, raw_name
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / raw_name).write_bytes(b"audio")

    audio_ranges, screen_ranges, segments = mod.scan_day("20240101")

    assert audio_ranges == [("09:00", "09:15")]
    assert screen_ranges == []
    assert segments[0]["types"] == ["audio"]
    assert segments[0]["data_state"] == {"audio": "pending"}


def test_scan_day_marks_header_only_audio_pending(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "audio.jsonl").write_text('{"raw": "audio.flac"}\n')

    audio_ranges, _, segments = mod.scan_day("20240101")

    assert audio_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"audio": "pending"}


def test_derive_modality_state_chunks_win_rescue(tmp_path):
    from solstone.think.data_state import derive_modality_state

    segment = tmp_path / "090000_300"
    segment.mkdir()
    marker = segment / ".analyzing_audio"
    marker.write_text('{"started_at": "2026-05-20T09:00:00Z", "modality": "audio"}\n')

    state = derive_modality_state(
        segment,
        "audio",
        has_chunks=True,
        has_jsonl=True,
        has_raw=True,
    )

    assert state == "analyzed"
    assert not marker.exists()


def test_derive_modality_state_stale_marker_renames_failed(tmp_path):
    from solstone.think.data_state import derive_modality_state

    segment = tmp_path / "090000_300"
    segment.mkdir()
    marker = segment / ".analyzing_screen"
    failed = segment / ".analyze_failed_screen"
    marker.write_text('{"started_at": "2026-05-20T09:00:00Z", "modality": "screen"}\n')
    old_time = time.time() - 2000
    os.utime(marker, (old_time, old_time))

    state = derive_modality_state(
        segment,
        "screen",
        has_chunks=False,
        has_jsonl=True,
        has_raw=True,
    )

    assert state == "failed"
    assert not marker.exists()
    payload = json.loads(failed.read_text())
    assert payload["reason"] == "stale"
    assert payload["modality"] == "screen"


def test_derive_modality_state_corrupt_marker_renames_failed(tmp_path):
    from solstone.think.data_state import derive_modality_state

    segment = tmp_path / "090000_300"
    segment.mkdir()
    marker = segment / ".analyzing_screen"
    failed = segment / ".analyze_failed_screen"
    marker.write_text("{not json")

    state = derive_modality_state(
        segment,
        "screen",
        has_chunks=False,
        has_jsonl=False,
        has_raw=True,
    )

    assert state == "failed"
    assert not marker.exists()
    payload = json.loads(failed.read_text())
    assert payload["reason"] == "marker_corrupt"
    assert payload["modality"] == "screen"


def test_derive_modality_state_does_not_probe_processes(tmp_path, monkeypatch):
    from solstone.think.data_state import derive_modality_state

    def fail_os_kill(pid, sig):  # pragma: no cover - fails if called
        raise AssertionError("os.kill should not be used for analyzing state")

    monkeypatch.setattr(os, "kill", fail_os_kill)
    segment = tmp_path / "090000_300"
    segment.mkdir()
    (segment / ".analyzing_screen").write_text(
        '{"started_at": "2026-05-20T09:00:00Z", "modality": "screen"}\n'
    )

    assert (
        derive_modality_state(
            segment,
            "screen",
            has_chunks=False,
            has_jsonl=True,
            has_raw=True,
        )
        == "analyzing"
    )


def test_scan_day_detects_analyzing_markers_from_fixture(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    source = Path("tests/fixtures/journal/chronicle/20260520")
    dest = day_path("20260520")
    shutil.copytree(source, dest, dirs_exist_ok=True)
    stale_marker = dest / "default" / "093000_300" / ".analyzing_screen"
    old_time = time.time() - 2000
    os.utime(stale_marker, (old_time, old_time))

    mod = importlib.import_module("solstone.think.cluster")

    _audio_ranges, _screen_ranges, segments = mod.scan_day("20260520")
    by_key = {segment["key"]: segment for segment in segments}

    assert by_key["090000_300"]["data_state"]["screen"] == "analyzing"
    assert by_key["091000_300"]["data_state"]["screen"] == "failed"
    assert by_key["092000_300"]["data_state"]["screen"] == "analyzed"
    assert not (dest / "default" / "092000_300" / ".analyzing_screen").exists()
    assert by_key["093000_300"]["data_state"]["screen"] == "failed"
    stale_payload = json.loads(
        (dest / "default" / "093000_300" / ".analyze_failed_screen").read_text()
    )
    assert stale_payload["reason"] == "stale"
    assert by_key["094000_300"]["data_state"] == {
        "audio": "pending",
        "screen": "pending",
    }


def test_scan_day_marks_analyzed_audio_analyzed(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "audio.jsonl").write_text(
        '{"raw": "audio.flac"}\n'
        '{"start": "00:00:01", "source": "mic", "text": "audio line"}\n'
    )

    audio_ranges, _, segments = mod.scan_day("20240101")

    assert audio_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"audio": "analyzed"}


def test_scan_day_keeps_audio_raw_substring_collision_pending(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / "audio.jsonl").write_text('{"raw": "startup_audio.flac"}\n')

    audio_ranges, _, segments = mod.scan_day("20240101")

    assert audio_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"audio": "pending"}


def test_scan_day_omits_absent_modalities(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)

    assert mod.scan_day("20240101") == ([], [], [])


@pytest.mark.parametrize("filename", ["imported.md", "call_transcript.md"])
def test_scan_day_marks_text_transcript_audio_analyzed(tmp_path, monkeypatch, filename):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = day_path("20240101")

    mod = importlib.import_module("solstone.think.cluster")

    segment = day_dir / "default" / "090000_300"
    segment.mkdir(parents=True)
    (segment / filename).write_text("transcript text\n")

    audio_ranges, _, segments = mod.scan_day("20240101")

    assert audio_ranges == [("09:00", "09:15")]
    assert segments[0]["data_state"] == {"audio": "analyzed"}


def test_day_path_create_false(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    missing = day_path("29990101", create=False)
    assert not missing.exists()

    created = day_path("29990101")
    assert created.exists()


def test_find_segment_dir_missing_streamed_segment_does_not_create_directory(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    mod = importlib.import_module("solstone.think.cluster")
    result = mod._find_segment_dir("29990101", "090000_300", "default")

    assert result is None
    assert not (tmp_path / "chronicle" / "29990101").exists()
