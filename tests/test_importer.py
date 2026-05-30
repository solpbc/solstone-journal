# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import datetime as dt
import hashlib
import importlib
import json
import subprocess
import time
import zipfile
from pathlib import Path
from unittest.mock import ANY, MagicMock, patch

import pytest

from solstone.think.importers.file_importer import ImportPreview, ImportResult
from solstone.think.utils import day_path


def _make_mock_file_importer(name="ics", display_name="ICS Calendar"):
    """Create a mock FileImporter for testing."""
    mock_imp = MagicMock()
    mock_imp.name = name
    mock_imp.display_name = display_name
    mock_imp.file_patterns = ["*.ics"]
    mock_imp.description = "Import calendar events from ICS files"

    mock_imp.preview.return_value = ImportPreview(
        date_range=("20250101", "20250301"),
        item_count=42,
        entity_count=5,
        summary="42 calendar events from 5 calendars",
    )
    mock_imp.process.return_value = ImportResult(
        entries_written=42,
        entities_seeded=5,
        files_created=["/journal/20250101/import.ics/imported.jsonl"],
        errors=[],
        summary="Imported 42 events",
    )
    return mock_imp


def _configure_text_import_runtime(monkeypatch, mod):
    """Patch text import processing and callosum helpers for CLI tests."""
    text_mod = importlib.import_module("solstone.think.importers.text")

    monkeypatch.setattr(
        text_mod,
        "detect_transcript_segment",
        lambda text, start_time: [("12:00:00", text)],
    )
    monkeypatch.setattr(
        text_mod,
        "detect_transcript_json",
        lambda text, segment_start: {
            "entries": [
                {
                    "start": segment_start,
                    "speaker": "Unknown",
                    "text": text,
                }
            ],
            "topics": "",
            "setting": "",
        },
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: MagicMock())
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)


def _read_action_entries(journal_root: Path) -> list[dict]:
    """Read journal-level app action log entries for today."""
    today = dt.datetime.now().strftime("%Y%m%d")
    log_path = journal_root / "config" / "actions" / f"{today}.jsonl"
    if not log_path.exists():
        return []
    return [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def test_slice_audio_segment(tmp_path):
    """Test slice_audio_segment extracts audio with stream copy."""
    mod = importlib.import_module("solstone.think.importers.audio")

    source = tmp_path / "source.mp3"
    source.write_bytes(b"fake audio")
    output = tmp_path / "segment.mp3"

    # Mock subprocess.run to simulate successful ffmpeg
    with patch("subprocess.run") as mock_run:
        mock_run.return_value = None

        result = mod.slice_audio_segment(str(source), str(output), 0, 300)

        assert result == str(output)
        # First call should use -c:a copy
        call_args = mock_run.call_args_list[0][0][0]
        assert "-c:a" in call_args
        assert "copy" in call_args


def test_slice_audio_segment_fallback(tmp_path):
    """Test slice_audio_segment falls back to re-encode on copy failure."""
    mod = importlib.import_module("solstone.think.importers.audio")

    source = tmp_path / "source.mp3"
    source.write_bytes(b"fake audio")
    output = tmp_path / "segment.mp3"

    # First call (copy) fails, second call (re-encode) succeeds
    call_count = [0]

    def mock_run(cmd, *args, **kwargs):
        call_count[0] += 1
        if call_count[0] == 1:
            # First call (stream copy) fails
            raise subprocess.CalledProcessError(1, cmd)
        # Second call (re-encode) succeeds
        return None

    with patch("subprocess.run", side_effect=mock_run):
        result = mod.slice_audio_segment(str(source), str(output), 0, 300)

        assert result == str(output)
        assert call_count[0] == 2  # Both attempts were made


def test_importer_text(tmp_path, monkeypatch):
    """Test importing a text transcript file."""
    mod = importlib.import_module("solstone.think.importers.cli")
    text_mod = importlib.import_module("solstone.think.importers.text")

    transcript = "hello\nworld"
    txt = tmp_path / "sample.txt"
    txt.write_text(transcript)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        mod, "detect_created", lambda p, **kw: {"day": "20240101", "time": "120000"}
    )

    # Mock segment detection: returns (start_at, text) tuples with absolute times
    def mock_detect_segment(text, start_time):
        return [("12:00:00", "seg1"), ("12:05:00", "seg2")]

    monkeypatch.setattr(text_mod, "detect_transcript_segment", mock_detect_segment)

    # Mock JSON conversion: returns entries with absolute timestamps
    def mock_detect_json(text, segment_start):
        return {
            "entries": [{"start": segment_start, "speaker": "Unknown", "text": text}],
            "topics": "",
            "setting": "",
        }

    monkeypatch.setattr(text_mod, "detect_transcript_json", mock_detect_json)

    # Mock CallosumConnection and status emitter to avoid real sockets/threads
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: MagicMock())
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--timestamp", "20240101_120000"],
    )
    mod.main()

    day_dir = day_path("20240101")
    # Duration: seg1 starts at 12:00:00, seg2 at 12:05:00 = 300s duration
    # Last segment (seg2) defaults to 5s since no audio duration
    # Segments are under stream directory (import.text for .txt files)
    f1 = day_dir / "import.text" / "120000_300" / "conversation_transcript.jsonl"
    f2 = day_dir / "import.text" / "120500_5" / "conversation_transcript.jsonl"

    # Read JSONL format: first line is metadata, subsequent lines are entries
    lines1 = f1.read_text().strip().split("\n")
    metadata1 = json.loads(lines1[0])
    entries1 = [json.loads(line) for line in lines1[1:]]

    lines2 = f2.read_text().strip().split("\n")
    metadata2 = json.loads(lines2[0])
    entries2 = [json.loads(line) for line in lines2[1:]]

    # Timestamps are relative offsets from segment start (not absolute time-of-day)
    assert entries1 == [
        {"start": "00:00:00", "speaker": "Unknown", "text": "seg1", "source": "import"}
    ]
    assert metadata1["imported"]["id"] == "20240101_120000"
    assert "facet" not in metadata1["imported"]
    # raw path should resolve from segment dir (3 levels deep) to imports/
    assert metadata1["raw"] == "../../../imports/20240101_120000/sample.txt"

    assert entries2 == [
        {"start": "00:00:00", "speaker": "Unknown", "text": "seg2", "source": "import"}
    ]
    assert metadata2["imported"]["id"] == "20240101_120000"
    assert "facet" not in metadata2["imported"]

    # segments.json should be written in the import directory
    segments_json = tmp_path / "imports" / "20240101_120000" / "segments.json"
    assert segments_json.exists()
    seg_data = json.loads(segments_json.read_text())
    assert seg_data["day"] == "20240101"
    assert "120000_300" in seg_data["segments"]
    assert "120500_5" in seg_data["segments"]

    # stream.json should be written in each segment directory
    stream1 = day_dir / "import.text" / "120000_300" / "stream.json"
    assert stream1.exists()
    stream1_data = json.loads(stream1.read_text())
    assert stream1_data["stream"] == "import.text"


def test_text_import_observed_events_are_batch_and_drain_once(tmp_path, monkeypatch):
    """Text imports mark observed segments as batch and queue one daily drain."""
    mod = importlib.import_module("solstone.think.importers.cli")
    text_mod = importlib.import_module("solstone.think.importers.text")
    from solstone.think.utils import updated_days

    txt = tmp_path / "sample.txt"
    txt.write_text("segment text")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        text_mod,
        "detect_transcript_segment",
        lambda text, start_time: [("12:00:00", text)],
    )
    monkeypatch.setattr(
        text_mod,
        "detect_transcript_json",
        lambda text, segment_start: {
            "entries": [
                {
                    "start": segment_start,
                    "speaker": "Unknown",
                    "text": text,
                }
            ],
            "topics": "",
            "setting": "",
        },
    )
    callosum = MagicMock()
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    health = tmp_path / "chronicle" / "20240101" / "health"
    health.mkdir(parents=True)
    (health / "daily.updated").touch()
    time.sleep(0.05)

    result = mod.import_one(txt, timestamp="20240101_120000")

    assert result is not None
    observed = [
        call
        for call in callosum.emit.call_args_list
        if call.args[:2] == ("observe", "observed")
    ]
    assert observed
    assert all(call.kwargs.get("batch") is True for call in observed)

    drain_days = [
        call.kwargs["day"]
        for call in callosum.emit.call_args_list
        if call.args[:2] == ("supervisor", "drain")
    ]
    assert drain_days == ["20240101"]
    assert "20240101" in updated_days()


def test_importer_pdf(tmp_path, monkeypatch):
    """Test importing a PDF transcript file."""
    mod = importlib.import_module("solstone.think.importers.cli")
    text_mod = importlib.import_module("solstone.think.importers.text")

    # Create a fake PDF file (content doesn't matter — pypdf is mocked)
    pdf = tmp_path / "meeting.pdf"
    pdf.write_bytes(b"%PDF-1.4 fake")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        mod, "detect_created", lambda p, **kw: {"day": "20251205", "time": "163000"}
    )

    # Mock _read_transcript to return extracted text (bypasses pypdf)
    monkeypatch.setattr(
        text_mod, "_read_transcript", lambda path: "Board meeting notes\nAction items"
    )

    # Mock segment detection: single segment for short text
    def mock_detect_segment(text, start_time):
        return [("16:30:00", text)]

    monkeypatch.setattr(text_mod, "detect_transcript_segment", mock_detect_segment)

    # Mock JSON conversion
    def mock_detect_json(text, segment_start):
        return {
            "entries": [
                {
                    "start": segment_start,
                    "speaker": "Jack",
                    "text": "Board meeting notes",
                },
                {"start": "16:30:30", "speaker": "Ramon", "text": "Action items"},
            ],
            "topics": "board meeting, action items",
            "setting": "workplace",
        }

    monkeypatch.setattr(text_mod, "detect_transcript_json", mock_detect_json)

    # Mock CallosumConnection and status emitter
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: MagicMock())
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    monkeypatch.setattr(
        "sys.argv",
        [
            "sol import",
            str(pdf),
            "--timestamp",
            "20251205_163000",
            "--facet",
            "work",
            "--setting",
            "board meeting",
        ],
    )
    mod.main()

    day_dir = day_path("20251205")
    # Single segment, last segment defaults to 5s
    f1 = day_dir / "import.text" / "163000_5" / "conversation_transcript.jsonl"
    assert f1.exists()

    lines = f1.read_text().strip().split("\n")
    metadata = json.loads(lines[0])
    entries = [json.loads(line) for line in lines[1:]]

    # Verify metadata
    assert metadata["imported"]["id"] == "20251205_163000"
    assert metadata["imported"]["facet"] == "work"
    assert metadata["imported"]["setting"] == "board meeting"
    assert metadata["raw"] == "../../../imports/20251205_163000/meeting.pdf"

    # Verify entries — timestamps are relative offsets, topics/setting in metadata
    assert entries[0] == {
        "start": "00:00:00",
        "speaker": "Jack",
        "text": "Board meeting notes",
        "source": "import",
    }
    assert entries[1] == {
        "start": "00:00:30",
        "speaker": "Ramon",
        "text": "Action items",
        "source": "import",
    }
    # Topics/setting extracted to metadata (not written as entry)
    assert len(entries) == 2
    assert metadata["topics"] == "board meeting, action items"
    assert metadata["setting"] == "workplace"

    # Verify .pdf auto-detected as text import (stream = import.text)
    stream_json = day_dir / "import.text" / "163000_5" / "stream.json"
    assert stream_json.exists()
    stream_data = json.loads(stream_json.read_text())
    assert stream_data["stream"] == "import.text"

    # Verify segments.json written
    segments_json = tmp_path / "imports" / "20251205_163000" / "segments.json"
    assert segments_json.exists()


def test_write_segment(tmp_path):
    """Test write_segment creates a segment directory and JSONL file."""
    mod = importlib.import_module("solstone.think.importers.shared")

    json_path = mod.write_segment(
        str(tmp_path / "chronicle" / "20240101"),
        "import.text",
        "120000_300",
        [{"start": "00:00:00", "speaker": "Alice", "text": "Hello"}],
        import_id="20240101_120000",
        raw_filename="notes.txt",
        facet="work",
        setting="standup",
        model="gpt-4",
    )

    written = Path(json_path)
    assert (
        written
        == tmp_path
        / "chronicle"
        / "20240101"
        / "import.text"
        / "120000_300"
        / "conversation_transcript.jsonl"
    )
    assert written.exists()

    lines = written.read_text().strip().split("\n")
    metadata = json.loads(lines[0])
    entry = json.loads(lines[1])

    assert metadata["imported"] == {
        "id": "20240101_120000",
        "facet": "work",
        "setting": "standup",
    }
    assert metadata["raw"] == "../../../imports/20240101_120000/notes.txt"
    assert metadata["model"] == "gpt-4"
    assert entry["source"] == "import"


def test_write_markdown_segments(tmp_path, monkeypatch):
    """write_markdown_segments creates segment dirs with imported.md files."""
    mod = importlib.import_module("solstone.think.importers.shared")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    windows = [
        ("20260301", "120000_300", [{"text": "hello"}, {"text": "world"}]),
        ("20260302", "090000_300", [{"text": "morning"}]),
    ]

    def render(items):
        return "\n\n".join(item["text"] for item in items)

    files, segments = mod.write_markdown_segments("test", windows, render)

    assert len(files) == 2
    assert len(segments) == 2

    first_md = day_path("20260301") / "import.test" / "120000_300" / "imported.md"
    assert first_md.exists()
    content = first_md.read_text()
    assert "hello" in content
    assert "world" in content
    assert content.endswith("\n")

    second_md = day_path("20260302") / "import.test" / "090000_300" / "imported.md"
    assert second_md.exists()

    assert segments == [("20260301", "120000_300"), ("20260302", "090000_300")]


def test_chatgpt_importer_segments(tmp_path, monkeypatch):
    """ChatGPT importer should write message windows as import segments."""
    mod = importlib.import_module("solstone.think.importers.chatgpt")

    base = dt.datetime(2026, 1, 15, 12, 0, 0).timestamp()
    conversations = [
        {
            "title": "First",
            "current_node": "a3",
            "mapping": {
                "a1": {
                    "parent": None,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"parts": ["Hello"]},
                        "create_time": base,
                    },
                },
                "a2": {
                    "parent": "a1",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"parts": ["Hi there"]},
                        "create_time": base + 60,
                        "metadata": {"model_slug": "gpt-4"},
                    },
                },
                "a3": {
                    "parent": "a2",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"parts": ["New topic"]},
                        "create_time": base + 301,
                    },
                },
            },
        },
        {
            "title": "Second",
            "current_node": "b2",
            "mapping": {
                "b1": {
                    "parent": None,
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"parts": ["Missing time"]},
                    },
                },
                "b2": {
                    "parent": "b1",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"parts": ["Next day reply"]},
                        "create_time": base + 12 * 3600,
                        "metadata": {"model_slug": "gpt-4o"},
                    },
                },
            },
        },
    ]

    archive = tmp_path / "chatgpt.zip"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("conversations.json", json.dumps(conversations))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    fixed_dt = dt.datetime(2026, 1, 20, 8, 30, 0)

    class FixedDateTime(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return fixed_dt

    monkeypatch.setattr(mod.dt, "datetime", FixedDateTime)

    result = mod.ChatGPTImporter().process(archive, tmp_path, facet="work")

    assert result.entries_written == 4
    assert result.errors == []
    assert result.segments == [
        ("20260115", "120000_300"),
        ("20260115", "120501_300"),
        ("20260116", "000000_300"),
    ]
    assert len(result.files_created) == 3

    first_segment = (
        day_path("20260115")
        / "import.chatgpt"
        / "120000_300"
        / "conversation_transcript.jsonl"
    )
    second_segment = (
        day_path("20260115")
        / "import.chatgpt"
        / "120501_300"
        / "conversation_transcript.jsonl"
    )
    third_segment = (
        day_path("20260116")
        / "import.chatgpt"
        / "000000_300"
        / "conversation_transcript.jsonl"
    )

    assert first_segment.exists()
    assert second_segment.exists()
    assert third_segment.exists()

    first_lines = first_segment.read_text().strip().split("\n")
    first_meta = json.loads(first_lines[0])
    first_entries = [json.loads(line) for line in first_lines[1:]]
    assert first_meta["imported"] == {"id": "20260120_083000", "facet": "work"}
    assert first_meta["model"] == "gpt-4"
    assert first_entries == [
        {"start": "00:00:00", "speaker": "Human", "text": "Hello", "source": "import"},
        {
            "start": "00:01:00",
            "speaker": "Assistant",
            "text": "Hi there",
            "source": "import",
        },
    ]

    second_lines = second_segment.read_text().strip().split("\n")
    second_meta = json.loads(second_lines[0])
    second_entries = [json.loads(line) for line in second_lines[1:]]
    assert second_meta == {"imported": {"id": "20260120_083000", "facet": "work"}}
    assert second_entries == [
        {
            "start": "00:00:00",
            "speaker": "Human",
            "text": "New topic",
            "source": "import",
        }
    ]

    third_lines = third_segment.read_text().strip().split("\n")
    third_meta = json.loads(third_lines[0])
    third_entries = [json.loads(line) for line in third_lines[1:]]
    assert third_meta["model"] == "gpt-4o"
    assert third_entries == [
        {
            "start": "00:00:00",
            "speaker": "Assistant",
            "text": "Next day reply",
            "source": "import",
        }
    ]


def test_claude_chat_importer_segments(tmp_path, monkeypatch):
    """Claude importer should write message windows as import segments."""
    mod = importlib.import_module("solstone.think.importers.claude_chat")

    base = dt.datetime(2026, 1, 15, 12, 0, 0)
    conversations = [
        {
            "name": "First",
            "created_at": base.isoformat(),
            "chat_messages": [
                {
                    "sender": "human",
                    "text": "Hello",
                    "created_at": base.isoformat(),
                },
                {
                    "sender": "assistant",
                    "text": "Hi there",
                    "created_at": (base + dt.timedelta(seconds=60)).isoformat(),
                },
                {
                    "sender": "human",
                    "text": "New topic",
                    "created_at": (base + dt.timedelta(seconds=301)).isoformat(),
                },
            ],
        },
        {
            "name": "Second",
            "created_at": (base + dt.timedelta(hours=12)).isoformat(),
            "chat_messages": [
                {
                    "sender": "assistant",
                    "text": "Fallback time",
                },
                {
                    "sender": "assistant",
                    "text": "Next day reply",
                    "created_at": (base + dt.timedelta(hours=12)).isoformat(),
                },
            ],
        },
        {
            "name": "Empty",
            "created_at": base.isoformat(),
            "chat_messages": [],
        },
    ]

    archive = tmp_path / "claude.zip"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("conversations.json", json.dumps(conversations))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    fixed_dt = dt.datetime(2026, 1, 20, 8, 30, 0)

    class FixedDateTime(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return fixed_dt

    monkeypatch.setattr(mod.dt, "datetime", FixedDateTime)

    result = mod.ClaudeChatImporter().process(archive, tmp_path, facet="work")

    assert result.entries_written == 5
    assert result.errors == []
    assert result.segments == [
        ("20260115", "120000_300"),
        ("20260115", "120501_300"),
        ("20260116", "000000_300"),
    ]
    assert len(result.files_created) == 3

    first_segment = (
        day_path("20260115")
        / "import.claude"
        / "120000_300"
        / "conversation_transcript.jsonl"
    )
    second_segment = (
        day_path("20260115")
        / "import.claude"
        / "120501_300"
        / "conversation_transcript.jsonl"
    )
    third_segment = (
        day_path("20260116")
        / "import.claude"
        / "000000_300"
        / "conversation_transcript.jsonl"
    )

    assert first_segment.exists()
    assert second_segment.exists()
    assert third_segment.exists()

    first_lines = first_segment.read_text().strip().split("\n")
    first_meta = json.loads(first_lines[0])
    first_entries = [json.loads(line) for line in first_lines[1:]]
    assert first_meta["imported"] == {"id": "20260120_083000", "facet": "work"}
    assert first_entries == [
        {"start": "00:00:00", "speaker": "Human", "text": "Hello", "source": "import"},
        {
            "start": "00:01:00",
            "speaker": "Assistant",
            "text": "Hi there",
            "source": "import",
        },
    ]

    second_lines = second_segment.read_text().strip().split("\n")
    second_meta = json.loads(second_lines[0])
    second_entries = [json.loads(line) for line in second_lines[1:]]
    assert second_meta == {"imported": {"id": "20260120_083000", "facet": "work"}}
    assert second_entries == [
        {
            "start": "00:00:00",
            "speaker": "Human",
            "text": "New topic",
            "source": "import",
        }
    ]

    third_lines = third_segment.read_text().strip().split("\n")
    third_meta = json.loads(third_lines[0])
    third_entries = [json.loads(line) for line in third_lines[1:]]
    assert third_meta == {"imported": {"id": "20260120_083000", "facet": "work"}}
    assert third_entries == [
        {
            "start": "00:00:00",
            "speaker": "Assistant",
            "text": "Fallback time",
            "source": "import",
        },
        {
            "start": "00:00:00",
            "speaker": "Assistant",
            "text": "Next day reply",
            "source": "import",
        },
    ]


def test_format_audio_stream_path():
    """Test format_audio correctly parses timestamps from stream-based paths."""
    from solstone.observe.hear import format_audio

    entries = [
        {"imported": {"id": "20240101_120000"}, "raw": "test.txt"},
        {"start": "12:00:00", "speaker": "Alice", "text": "Hello"},
        {"start": "12:00:30", "speaker": "Bob", "text": "Hi there"},
    ]

    # Stream-based path: day/stream/segment/conversation_transcript.jsonl
    context = {
        "file_path": Path(
            "/journal/20240101/import.text/120000_300/conversation_transcript.jsonl"
        )
    }
    chunks, meta = format_audio(entries, context)

    assert len(chunks) == 2
    # Verify timestamps are non-zero (base_timestamp correctly parsed from path)
    assert chunks[0]["timestamp"] > 0
    assert chunks[1]["timestamp"] > chunks[0]["timestamp"]
    # Verify header includes start time
    assert meta.get("header") and "12:00" in meta["header"]


def test_format_audio_legacy_path():
    """Test format_audio still works with legacy day/segment/ paths."""
    from solstone.observe.hear import format_audio

    entries = [
        {"raw": "raw.flac", "model": "whisper-1"},
        {"start": "12:34:56", "source": "mic", "text": "Test"},
    ]

    # Legacy path: day/segment/audio.jsonl (no stream directory)
    context = {"file_path": Path("/journal/20240101/123456_300/audio.jsonl")}
    chunks, meta = format_audio(entries, context)

    assert len(chunks) == 1
    assert chunks[0]["timestamp"] > 0


def test_get_audio_duration(tmp_path):
    """Test _get_audio_duration calls ffprobe correctly."""
    mod = importlib.import_module("solstone.think.importers.audio")

    audio_file = tmp_path / "test.mp3"
    audio_file.write_bytes(b"fake audio")

    # Mock ffprobe returning duration
    mock_result = MagicMock()
    mock_result.stdout = "123.456\n"

    with patch("subprocess.run", return_value=mock_result) as mock_run:
        duration = mod._get_audio_duration(str(audio_file))

        assert duration == 123.456
        # Verify ffprobe was called with correct args
        call_args = mock_run.call_args[0][0]
        assert "ffprobe" in call_args
        assert str(audio_file) in call_args


def test_get_audio_duration_failure(tmp_path):
    """Test _get_audio_duration returns None on error."""
    mod = importlib.import_module("solstone.think.importers.audio")

    audio_file = tmp_path / "test.mp3"
    audio_file.write_bytes(b"fake audio")

    with patch(
        "subprocess.run", side_effect=subprocess.CalledProcessError(1, "ffprobe")
    ):
        duration = mod._get_audio_duration(str(audio_file))
        assert duration is None


def test_prepare_audio_segments(tmp_path, monkeypatch):
    """Test prepare_audio_segments creates segment directories with audio slices."""
    mod = importlib.import_module("solstone.think.importers.audio")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    audio_file = tmp_path / "test.mp3"
    audio_file.write_bytes(b"fake audio content")

    day_dir = tmp_path / "chronicle" / "20240101"
    day_dir.mkdir(parents=True)

    base_dt = dt.datetime(2024, 1, 1, 12, 0, 0)

    # Mock _get_audio_duration to return 7 minutes (2.33 segments)
    monkeypatch.setattr(mod, "_get_audio_duration", lambda p: 420.0)

    # Mock slice_audio_segment to create the file
    def mock_slice(src, dst, start, duration):
        Path(dst).parent.mkdir(parents=True, exist_ok=True)
        Path(dst).write_bytes(b"sliced audio")
        return dst

    monkeypatch.setattr(mod, "slice_audio_segment", mock_slice)

    # Mock find_available_segment to return segment as-is (no collision)
    monkeypatch.setattr(mod, "find_available_segment", lambda day, seg: seg)

    segments = mod.prepare_audio_segments(
        str(audio_file),
        str(day_dir),
        base_dt,
        "20240101_120000",
        "import.audio",
    )

    # Should create 2 segments (0-5 min, 5-7 min)
    assert len(segments) == 2

    seg1_key, seg1_dir, seg1_files = segments[0]
    assert seg1_key == "120000_300"
    assert seg1_files == ["imported_audio.mp3"]
    assert (seg1_dir / "imported_audio.mp3").exists()
    # Segment should be under stream directory
    assert seg1_dir == day_dir / "import.audio" / "120000_300"

    seg2_key, seg2_dir, seg2_files = segments[1]
    assert seg2_key == "120500_300"
    assert seg2_files == ["imported_audio.mp3"]
    assert (seg2_dir / "imported_audio.mp3").exists()
    assert seg2_dir == day_dir / "import.audio" / "120500_300"


def test_prepare_audio_segments_with_collision(tmp_path, monkeypatch):
    """Test prepare_audio_segments handles segment key collisions."""
    mod = importlib.import_module("solstone.think.importers.audio")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    audio_file = tmp_path / "test.mp3"
    audio_file.write_bytes(b"fake audio content")

    day_dir = tmp_path / "chronicle" / "20240101"
    day_dir.mkdir(parents=True)

    base_dt = dt.datetime(2024, 1, 1, 12, 0, 0)

    monkeypatch.setattr(mod, "_get_audio_duration", lambda p: 300.0)

    def mock_slice(src, dst, start, duration):
        Path(dst).parent.mkdir(parents=True, exist_ok=True)
        Path(dst).write_bytes(b"sliced audio")
        return dst

    monkeypatch.setattr(mod, "slice_audio_segment", mock_slice)

    # Simulate collision - return modified segment key
    def mock_find_available(day, seg):
        if seg == "120000_300":
            return "120001_300"  # Deconflicted
        return seg

    monkeypatch.setattr(mod, "find_available_segment", mock_find_available)

    segments = mod.prepare_audio_segments(
        str(audio_file),
        str(day_dir),
        base_dt,
        "20240101_120000",
        "import.audio",
    )

    assert len(segments) == 1
    seg_key, seg_dir, seg_files = segments[0]
    assert seg_key == "120001_300"  # Deconflicted key
    assert seg_dir == day_dir / "import.audio" / "120001_300"


def test_importer_dry_run_text(tmp_path, monkeypatch, capsys):
    """Test --dry-run for text import prints plan without writing files."""
    mod = importlib.import_module("solstone.think.importers.cli")

    txt = tmp_path / "sample.txt"
    txt.write_text("hello\nworld\n")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--timestamp", "20240101_120000", "--dry-run"],
    )

    mod.main()

    captured = capsys.readouterr()
    assert "File:" in captured.out
    assert "Size:" in captured.out
    assert "Timestamp:" in captured.out
    assert "Source:" in captured.out
    assert "Stream:" in captured.out
    assert "Target day:" in captured.out
    assert "Content:" in captured.out
    assert "characters" in captured.out
    assert "lines" in captured.out
    assert "import.text" in captured.out
    assert "20240101" in captured.out
    assert "12 characters" in captured.out
    assert "2 lines" in captured.out

    assert not (tmp_path / "imports").exists()
    assert not (tmp_path / "chronicle" / "20240101").exists()


def test_importer_dry_run_audio(tmp_path, monkeypatch, capsys):
    """Test --dry-run for audio import prints plan without writing files."""
    mod = importlib.import_module("solstone.think.importers.cli")

    mp3 = tmp_path / "sample.mp3"
    mp3.write_bytes(b"fake audio")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(mod, "_get_audio_duration", lambda p: 420.0)
    callosum_cls = MagicMock()
    monkeypatch.setattr(mod, "CallosumConnection", callosum_cls)
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(mp3), "--timestamp", "20240101_120000", "--dry-run"],
    )

    mod.main()

    captured = capsys.readouterr()
    assert "File:" in captured.out
    assert "Size:" in captured.out
    assert "Timestamp:" in captured.out
    assert "Source:" in captured.out
    assert "Stream:" in captured.out
    assert "Target day:" in captured.out
    assert "Duration:" in captured.out
    assert "Segments:" in captured.out
    assert "Keys:" in captured.out
    assert "import.audio" in captured.out
    assert "20240101" in captured.out
    assert "7.0 minutes" in captured.out
    assert "2 (5-minute chunks)" in captured.out
    assert "120000_300" in captured.out
    assert "120500_300" in captured.out

    assert not (tmp_path / "imports").exists()
    assert not (tmp_path / "chronicle" / "20240101").exists()
    assert callosum_cls.call_count == 0


def test_importer_dry_run_auto(tmp_path, monkeypatch, capsys):
    """Test --dry-run with --auto detects timestamp and prints summary."""
    mod = importlib.import_module("solstone.think.importers.cli")

    txt = tmp_path / "notes.txt"
    txt.write_text("meeting notes")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        mod, "detect_created", lambda p, **kw: {"day": "20240315", "time": "140000"}
    )
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--auto", "--dry-run"],
    )

    mod.main()

    captured = capsys.readouterr()
    assert "Detected timestamp: 20240315_140000" in captured.out
    assert "auto-importing" in captured.out
    assert "import.text" in captured.out
    assert "Target day: 20240315" in captured.out
    assert "Content:" in captured.out

    assert not (tmp_path / "imports").exists()
    assert not (tmp_path / "chronicle" / "20240315").exists()


def test_importer_force_reimport_logs_manifest_and_replaces_directory(
    tmp_path, monkeypatch
):
    """--force logs a manifest, removes the old import dir, and writes the new file."""
    mod = importlib.import_module("solstone.think.importers.cli")

    timestamp = "20240101_120000"
    old_import_dir = tmp_path / "imports" / timestamp
    old_import_dir.mkdir(parents=True)
    stale_file = old_import_dir / "stale.txt"
    stale_bytes = b"old import payload"
    stale_file.write_bytes(stale_bytes)
    nested_file = old_import_dir / "nested" / "extra.bin"
    nested_file.parent.mkdir(parents=True)
    nested_bytes = b"\x00\x01nested"
    nested_file.write_bytes(nested_bytes)

    txt = tmp_path / "replacement.txt"
    txt.write_text("replacement transcript")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _configure_text_import_runtime(monkeypatch, mod)
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--timestamp", timestamp, "--force"],
    )

    mod.main()

    action_entries = _read_action_entries(tmp_path)
    assert len(action_entries) == 1
    entry = action_entries[0]
    assert entry["action"] == "import_force_reimport"
    assert entry["actor"] == "import"
    assert entry["params"]["dry_run"] is False
    assert entry["params"]["import_dir"] == str(old_import_dir)
    assert entry["params"]["file_count"] == 2
    assert entry["params"]["total_bytes"] == len(stale_bytes) + len(nested_bytes)
    assert entry["params"]["files"] == [
        {
            "name": "nested/extra.bin",
            "bytes": len(nested_bytes),
            "hash": hashlib.sha256(nested_bytes).hexdigest(),
        },
        {
            "name": "stale.txt",
            "bytes": len(stale_bytes),
            "hash": hashlib.sha256(stale_bytes).hexdigest(),
        },
    ]

    assert not stale_file.exists()
    assert not nested_file.exists()
    new_imported_file = old_import_dir / "replacement.txt"
    assert new_imported_file.exists()
    assert new_imported_file.read_text() == "replacement transcript"


def test_importer_force_dry_run_logs_manifest_without_deleting(tmp_path, monkeypatch):
    """--force --dry-run logs the manifest but leaves the old import dir untouched."""
    mod = importlib.import_module("solstone.think.importers.cli")

    timestamp = "20240101_120000"
    old_import_dir = tmp_path / "imports" / timestamp
    old_import_dir.mkdir(parents=True)
    stale_file = old_import_dir / "stale.txt"
    stale_file.write_text("old import payload")

    txt = tmp_path / "replacement.txt"
    txt.write_text("replacement transcript")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--timestamp", timestamp, "--force", "--dry-run"],
    )

    mod.main()

    action_entries = _read_action_entries(tmp_path)
    assert len(action_entries) == 1
    entry = action_entries[0]
    assert entry["action"] == "import_force_reimport"
    assert entry["params"]["dry_run"] is True
    assert entry["params"]["import_dir"] == str(old_import_dir)
    assert entry["params"]["file_count"] == 1
    assert stale_file.exists()
    assert stale_file.read_text() == "old import payload"
    assert not (old_import_dir / "replacement.txt").exists()


def test_importer_existing_import_without_force_still_errors(tmp_path, monkeypatch):
    """Existing imports still error without --force and do not log a reimport action."""
    mod = importlib.import_module("solstone.think.importers.cli")

    timestamp = "20240101_120000"
    existing_import_dir = tmp_path / "imports" / timestamp
    existing_import_dir.mkdir(parents=True)
    (existing_import_dir / "stale.txt").write_text("old import payload")

    txt = tmp_path / "replacement.txt"
    txt.write_text("replacement transcript")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(txt), "--timestamp", timestamp],
    )

    with pytest.raises(SystemExit, match="Import already exists"):
        mod.main()

    assert _read_action_entries(tmp_path) == []


def test_file_importer_without_timestamp(tmp_path, monkeypatch, capsys):
    """File importers auto-generate timestamp and skip import setup."""
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    fixed_dt = dt.datetime(2026, 3, 3, 12, 34, 56)

    class FixedDateTime(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return fixed_dt

    monkeypatch.setattr(mod.dt, "datetime", FixedDateTime)

    mock_imp = _make_mock_file_importer()
    callosum = MagicMock()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["sol import", str(ics_file), "--source", "ics"])
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    mod.main()

    mock_imp.process.assert_called_once_with(
        Path(ics_file),
        Path(tmp_path),
        facet=None,
        import_id="20260303_123456",
        progress_callback=ANY,
    )
    mock_call = callosum.emit.call_args_list[0]
    assert mock_call.args[0] == "importer"
    assert mock_call.args[1] == "started"
    assert mock_call.kwargs["import_id"] == "20260303_123456"
    # Manifest written for dedup tracking
    assert (tmp_path / "imports" / "20260303_123456" / "manifest.json").exists()


def test_file_importer_with_timestamp(tmp_path, monkeypatch):
    """File importer uses provided --timestamp and still skips import setup."""
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    mock_imp = _make_mock_file_importer()
    callosum = MagicMock()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol import",
            str(ics_file),
            "--source",
            "ics",
            "--timestamp",
            "20260303_120000",
        ],
    )
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    mod.main()

    mock_imp.process.assert_called_once_with(
        Path(ics_file),
        Path(tmp_path),
        facet=None,
        import_id="20260303_120000",
        progress_callback=ANY,
    )
    mock_call = callosum.emit.call_args_list[0]
    assert mock_call.args[0] == "importer"
    assert mock_call.args[1] == "started"
    assert mock_call.kwargs["import_id"] == "20260303_120000"
    # File importers write a manifest (but not source files) in imports/
    assert (tmp_path / "imports" / "20260303_120000" / "manifest.json").exists()


def test_file_importer_observed_events_are_batch_and_drain_distinct_days(
    tmp_path, monkeypatch
):
    """File imports queue one drain per distinct imported day."""
    mod = importlib.import_module("solstone.think.importers.cli")
    from solstone.think.utils import updated_days

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    mock_imp = _make_mock_file_importer()
    mock_imp.process.return_value = ImportResult(
        entries_written=3,
        entities_seeded=0,
        files_created=[],
        errors=[],
        summary="Imported 3 events",
        segments=[
            ("20260101", "120000_300"),
            ("20260101", "120500_300"),
            ("20260102", "090000_300"),
        ],
        date_range=("20260101", "20260102"),
    )
    callosum = MagicMock()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    for day in ("20260101", "20260102"):
        health = tmp_path / "chronicle" / day / "health"
        health.mkdir(parents=True)
        (health / "daily.updated").touch()
    time.sleep(0.05)

    result = mod.import_one(
        ics_file,
        source="ics",
        timestamp="20260303_120000",
    )

    assert result is not None
    observed = [
        call
        for call in callosum.emit.call_args_list
        if call.args[:2] == ("observe", "observed")
    ]
    assert len(observed) == 3
    assert all(call.kwargs.get("batch") is True for call in observed)

    drain_days = [
        call.kwargs["day"]
        for call in callosum.emit.call_args_list
        if call.args[:2] == ("supervisor", "drain")
    ]
    assert drain_days == ["20260101", "20260102"]
    assert {"20260101", "20260102"} <= set(updated_days())


def test_import_one_returns_metadata(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    mock_imp = _make_mock_file_importer()
    callosum = MagicMock()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)
    monkeypatch.setattr(mod, "index_file", lambda journal, file_path: True)

    result = mod.import_one(
        ics_file,
        source="ics",
        timestamp="20260303_120000",
    )

    assert result is not None
    assert result["processed_timestamp"] == "20260303_120000"
    assert result["entries_written"] == 42
    assert result["entities_seeded"] == 5
    assert result["all_created_files"] == [
        "/journal/20250101/import.ics/imported.jsonl"
    ]
    assert result["source_type"] == "ics"


def test_import_one_invalid_timestamp_raises_value_error(tmp_path):
    mod = importlib.import_module("solstone.think.importers.cli")
    media = tmp_path / "note.txt"
    media.write_text("hello", encoding="utf-8")

    with pytest.raises(ValueError, match="timestamp must be in YYYYMMDD_HHMMSS format"):
        mod.import_one(media, timestamp="not-a-timestamp")


def test_import_one_skips_wait_when_disabled(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.cli")

    audio_file = tmp_path / "test.mp3"
    audio_file.write_bytes(b"fake audio")
    callosum = MagicMock()

    def fake_prepare_audio_segments(media_path, day_dir, base_dt, import_id, stream):
        seg_dir = Path(day_dir) / stream / "120000_300"
        seg_dir.mkdir(parents=True, exist_ok=True)
        (seg_dir / "imported_audio.mp3").write_bytes(b"sliced audio")
        return [("120000_300", seg_dir, ["imported_audio.mp3"])]

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)
    monkeypatch.setattr(mod, "prepare_audio_segments", fake_prepare_audio_segments)
    monkeypatch.setattr(
        mod,
        "update_stream",
        lambda stream, day, seg, **kwargs: {
            "prev_day": None,
            "prev_segment": None,
            "seq": 1,
        },
    )
    monkeypatch.setattr(mod, "write_segment_stream", lambda *args, **kwargs: None)

    start = time.monotonic()
    result = mod.import_one(
        audio_file,
        timestamp="20260303_120000",
        source="audio",
        wait_for_processing=False,
    )
    elapsed = time.monotonic() - start

    assert result is not None
    assert elapsed < 5
    assert result.get("segments")
    assert "failed_segments" not in result


def test_file_importer_indexes_created_files_in_process(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    created_files = [
        str(tmp_path / "chronicle" / "20250101" / "import.ics" / "one.md"),
        str(tmp_path / "chronicle" / "20250102" / "import.ics" / "two.md"),
    ]
    mock_imp = _make_mock_file_importer()
    mock_imp.process.return_value = ImportResult(
        entries_written=2,
        entities_seeded=0,
        files_created=created_files,
        errors=[],
        summary="Imported 2 events",
    )
    callosum = MagicMock()
    index_mock = MagicMock(return_value=True)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)
    monkeypatch.setattr(mod, "index_file", index_mock)

    mod.import_one(
        ics_file,
        source="ics",
        timestamp="20260303_120000",
    )

    assert [call.args for call in index_mock.call_args_list] == [
        (str(tmp_path), created_files[0]),
        (str(tmp_path), created_files[1]),
    ]


def test_ics_creation_timestamp_last_modified():
    mod = importlib.import_module("solstone.think.importers.ics")
    icalendar = importlib.import_module("icalendar")

    ics_bytes = b"""BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
DTSTART:20260315T100000Z
LAST-MODIFIED:20260301T120500Z
END:VEVENT
END:VCALENDAR"""
    cal = icalendar.Calendar.from_ical(ics_bytes)
    component = list(cal.walk("VEVENT"))[0]

    assert (
        mod._creation_timestamp(component)
        == dt.datetime(2026, 3, 1, 12, 5, 0, tzinfo=dt.timezone.utc).timestamp()
    )


def test_ics_creation_timestamp_created_only():
    mod = importlib.import_module("solstone.think.importers.ics")
    icalendar = importlib.import_module("icalendar")

    ics_bytes = b"""BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
DTSTART:20260315T100000Z
CREATED:20260301T120000Z
END:VEVENT
END:VCALENDAR"""
    cal = icalendar.Calendar.from_ical(ics_bytes)
    component = list(cal.walk("VEVENT"))[0]

    assert (
        mod._creation_timestamp(component)
        == dt.datetime(2026, 3, 1, 12, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    )


def test_ics_creation_timestamp_dtstart_fallback():
    mod = importlib.import_module("solstone.think.importers.ics")
    icalendar = importlib.import_module("icalendar")

    ics_bytes = b"""BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
DTSTART:20260315T100000Z
END:VEVENT
END:VCALENDAR"""
    cal = icalendar.Calendar.from_ical(ics_bytes)
    component = list(cal.walk("VEVENT"))[0]

    assert (
        mod._creation_timestamp(component)
        == dt.datetime(2026, 3, 15, 10, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    )


def test_ics_creation_timestamp_none():
    mod = importlib.import_module("solstone.think.importers.ics")

    class EmptyComponent:
        def get(self, key, default=None):
            return default

    assert mod._creation_timestamp(EmptyComponent()) is None


def test_window_items_single_window():
    mod = importlib.import_module("solstone.think.importers.shared")

    base = dt.datetime(2026, 3, 1, 12, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    events = [
        {"title": "A", "create_ts": base},
        {"title": "B", "create_ts": base + 60},
        {"title": "C", "create_ts": base + 120},
    ]

    windows = mod.window_items(events, "create_ts")

    assert windows == [("20260301", "120000_300", events)]


def test_window_items_time_gap_split():
    mod = importlib.import_module("solstone.think.importers.shared")

    base = dt.datetime(2026, 3, 1, 12, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    events = [
        {"title": "A", "create_ts": base},
        {"title": "B", "create_ts": base + 60},
        {"title": "C", "create_ts": base + 120},
        {"title": "D", "create_ts": base + 600},
    ]

    windows = mod.window_items(events, "create_ts")

    assert len(windows) == 2
    assert windows[0][0] == "20260301"
    assert windows[0][1] == "120000_300"
    assert windows[0][2] == events[:3]
    assert windows[1][1] == "121000_300"
    assert windows[1][2] == [events[3]]


def test_window_items_day_boundary():
    mod = importlib.import_module("solstone.think.importers.shared")

    first_day = dt.datetime(2026, 3, 1, 12, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    second_day = dt.datetime(2026, 3, 2, 12, 0, 0, tzinfo=dt.timezone.utc).timestamp()
    events = [
        {"title": "A", "create_ts": first_day},
        {"title": "B", "create_ts": second_day},
    ]

    windows = mod.window_items(events, "create_ts")

    assert windows == [
        ("20260301", "120000_300", [events[0]]),
        ("20260302", "120000_300", [events[1]]),
    ]


def test_ics_render_event_markdown_full():
    mod = importlib.import_module("solstone.think.importers.ics")

    event = {
        "title": "Team Sync",
        "ts": "2026-01-15T10:00:00+00:00",
        "end_ts": "2026-01-15T11:00:00+00:00",
        "duration_minutes": 60,
        "location": "Conference Room 3B",
        "attendees": [
            {"name": "Alice Smith", "email": "alice@example.com"},
            {"name": "Bob Jones", "email": "bob@example.com"},
        ],
        "content": "Event description text here.",
    }

    rendered = mod._render_event_markdown(event)

    assert "## Team Sync" in rendered
    assert "**2026-01-15 10:00 AM – 11:00 AM** (60 min)" in rendered
    assert "📍 Conference Room 3B" in rendered
    assert "👥 Alice Smith, Bob Jones" in rendered
    assert "Event description text here." in rendered


def test_ics_render_event_markdown_minimal():
    mod = importlib.import_module("solstone.think.importers.ics")

    event = {
        "title": "Minimal Event",
        "ts": "2026-01-15T10:00:00+00:00",
        "content": "",
        "attendees": [],
    }

    rendered = mod._render_event_markdown(event)

    assert "## Minimal Event" in rendered
    assert "**2026-01-15 10:00 AM**" in rendered
    assert "📍" not in rendered
    assert "👥" not in rendered
    assert "Minimal Event\n\n" not in rendered


def test_ics_render_event_markdown_without_scheduled_time():
    mod = importlib.import_module("solstone.think.importers.ics")

    event = {
        "title": "Created Only Event",
        "content": "",
        "attendees": [],
    }

    rendered = mod._render_event_markdown(event)

    assert rendered == "## Created Only Event"


def test_ics_render_event_markdown_with_recurrence():
    mod = importlib.import_module("solstone.think.importers.ics")
    event = {
        "title": "Weekly Standup",
        "ts": "2026-03-15T11:00:00+00:00",
        "end_ts": "2026-03-15T11:30:00+00:00",
        "duration_minutes": 30,
        "recurrence": "Weekly on Mon",
    }
    rendered = mod._render_event_markdown(event)
    assert "## Weekly Standup" in rendered
    assert "🔁 Weekly on Mon" in rendered


def test_ics_describe_rrule_weekly_byday():
    mod = importlib.import_module("solstone.think.importers.ics")
    rrule = {"FREQ": ["WEEKLY"], "BYDAY": ["MO", "WE", "FR"]}
    assert mod._describe_rrule(rrule) == "Weekly on Mon, Wed, Fri"


def test_ics_describe_rrule_daily_interval():
    mod = importlib.import_module("solstone.think.importers.ics")
    rrule = {"FREQ": ["DAILY"], "INTERVAL": [2]}
    assert mod._describe_rrule(rrule) == "Every 2 days"


def test_ics_describe_rrule_monthly_byday():
    mod = importlib.import_module("solstone.think.importers.ics")
    rrule = {"FREQ": ["MONTHLY"], "BYDAY": ["TU"]}
    assert mod._describe_rrule(rrule) == "Monthly on Tue"


def test_ics_describe_rrule_with_count():
    mod = importlib.import_module("solstone.think.importers.ics")
    rrule = {"FREQ": ["WEEKLY"], "BYDAY": ["MO"], "COUNT": [10]}
    assert mod._describe_rrule(rrule) == "Weekly on Mon, 10 times"


def test_ics_describe_rrule_with_until():
    mod = importlib.import_module("solstone.think.importers.ics")
    until_dt = dt.datetime(2026, 12, 31, tzinfo=dt.timezone.utc)
    rrule = {
        "FREQ": ["WEEKLY"],
        "BYDAY": ["MO", "WE", "FR"],
        "UNTIL": [until_dt],
    }
    assert mod._describe_rrule(rrule) == "Weekly on Mon, Wed, Fri, until 2026-12-31"


def test_ics_describe_rrule_yearly():
    mod = importlib.import_module("solstone.think.importers.ics")
    rrule = {"FREQ": ["YEARLY"], "BYMONTH": [3], "BYMONTHDAY": [15]}
    assert mod._describe_rrule(rrule) == "Yearly on day 15 in Mar"


def test_ics_describe_rrule_empty():
    mod = importlib.import_module("solstone.think.importers.ics")
    assert mod._describe_rrule({}) == ""


def test_ics_process_segments(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.ics")

    ics_path = tmp_path / "calendar.ics"
    ics_path.write_bytes(
        b"""BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
DTSTART:20260315T100000Z
DTEND:20260315T110000Z
SUMMARY:Event One
DESCRIPTION:First description
CREATED:20260301T120000Z
ATTENDEE;CN=Alice Smith:mailto:alice@example.com
END:VEVENT
BEGIN:VEVENT
DTSTART:20260316T140000Z
DTEND:20260316T143000Z
SUMMARY:Event Two
CREATED:20260301T120200Z
END:VEVENT
BEGIN:VEVENT
DTSTART:20260315T110000Z
DTEND:20260315T113000Z
SUMMARY:Weekly Standup
CREATED:20260301T120100Z
RRULE:FREQ=WEEKLY;BYDAY=MO
END:VEVENT
BEGIN:VEVENT
DTSTART:20260317T090000Z
DTEND:20260317T093000Z
SUMMARY:Event Three
CREATED:20260302T090000Z
END:VEVENT
END:VCALENDAR"""
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = mod.ICSImporter().process(ics_path, tmp_path, facet="work")

    first_md = (
        day_path("20260301") / "import.ics" / "120000_300" / "event_transcript.md"
    )
    second_md = (
        day_path("20260302") / "import.ics" / "090000_300" / "event_transcript.md"
    )

    assert result.entries_written == 4
    assert result.errors == []
    assert result.segments == [
        ("20260301", "120000_300"),
        ("20260302", "090000_300"),
    ]
    assert len(result.files_created) == 2
    assert first_md.exists()
    assert second_md.exists()
    first_content = first_md.read_text()
    second_content = second_md.read_text()
    assert "## Event One" in first_content
    assert "First description" in first_content
    assert "🔁 Weekly on Mon" in first_content
    assert "## Event Two" in first_content
    assert "## Event Three" in second_content
    assert "**2026-03-17 09:00 AM – 09:30 AM** (30 min)" in second_content


def test_ics_preview_uses_creation_timestamps(tmp_path):
    mod = importlib.import_module("solstone.think.importers.ics")

    ics_path = tmp_path / "calendar.ics"
    ics_path.write_bytes(
        b"""BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
DTSTART:20260315T100000Z
DTEND:20260315T110000Z
SUMMARY:Event One
CREATED:20260301T120000Z
ATTENDEE;CN=Alice Smith:mailto:alice@example.com
END:VEVENT
BEGIN:VEVENT
DTSTART:20260316T100000Z
DTEND:20260316T110000Z
SUMMARY:Event Two
CREATED:20260305T090000Z
ATTENDEE;CN=Bob Jones:mailto:bob@example.com
END:VEVENT
END:VCALENDAR"""
    )

    preview = mod.ICSImporter().preview(ics_path)

    assert preview.date_range == ("20260301", "20260305")
    assert preview.item_count == 2
    assert preview.entity_count == 2
    assert preview.summary == "2 events, 2 unique attendees"


def test_list_importers_json(capsys, monkeypatch):
    """--list-importers --json returns machine-readable output."""
    mod = importlib.import_module("solstone.think.importers.cli")

    mock_imp = _make_mock_file_importer()
    monkeypatch.setattr("sys.argv", ["sol import", "--list-importers", "--json"])
    with patch(
        "solstone.think.importers.file_importer.get_file_importers",
        return_value=[mock_imp],
    ):
        mod.main()

    captured = capsys.readouterr()
    data = json.loads(captured.out)
    assert isinstance(data, list)
    assert len(data) == 1
    assert data[0]["name"] == "ics"
    assert "display_name" in data[0]
    assert "file_patterns" in data[0]
    assert "description" in data[0]


def test_dry_run_file_importer_json(tmp_path, monkeypatch, capsys):
    """--dry-run --json for file importer returns JSON metadata."""
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    mock_imp = _make_mock_file_importer()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(ics_file), "--source", "ics", "--dry-run", "--json"],
    )
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )

    mod.main()

    captured = capsys.readouterr()
    data = json.loads(captured.out)
    assert data["importer"] == "ics"
    assert data["source"] == str(ics_file)
    assert data["item_count"] == 42
    assert data["entity_count"] == 5
    assert data["summary"] == "42 calendar events from 5 calendars"
    assert isinstance(data["date_range"], list)
    mock_imp.process.assert_not_called()


def test_file_import_json(tmp_path, monkeypatch, capsys):
    """File importer prints machine-readable completion output."""
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")
    fixed_dt = dt.datetime(2026, 3, 3, 12, 34, 56)

    class FixedDateTime(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return fixed_dt

    monkeypatch.setattr(mod.dt, "datetime", FixedDateTime)

    mock_imp = _make_mock_file_importer()
    callosum = MagicMock()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(ics_file), "--source", "ics", "--json"],
    )
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    mod.main()

    captured = capsys.readouterr()
    data = json.loads(captured.out)
    assert data["importer"] == "ics"
    assert data["entries_written"] == 42
    assert data["entities_seeded"] == 5
    assert data["files_created"] == ["/journal/20250101/import.ics/imported.jsonl"]
    assert data["errors"] == []
    assert data["summary"] == "Imported 42 events"
    # Manifest written for dedup tracking
    assert (tmp_path / "imports").exists()


def test_file_importer_writes_manifest(tmp_path, monkeypatch):
    """File importers write a dedup manifest but don't copy source files to imports/."""
    mod = importlib.import_module("solstone.think.importers.cli")

    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR")

    mock_imp = _make_mock_file_importer()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(ics_file), "--source", "ics"],
    )
    monkeypatch.setattr(
        "solstone.think.importers.file_importer.get_file_importer",
        lambda name: mock_imp,
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: MagicMock())
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)

    mod.main()

    # Manifest exists, but source file was not copied into imports/
    imports_dir = tmp_path / "imports"
    assert imports_dir.exists()
    manifests = list(imports_dir.rglob("manifest.json"))
    assert len(manifests) == 1
    # No import.json (legacy audio import metadata)
    assert not list(imports_dir.rglob("import.json"))


def test_obsidian_process_segments(tmp_path, monkeypatch):
    """Obsidian importer writes creation-moment segments with markdown output."""
    mod = importlib.import_module("solstone.think.importers.obsidian")

    vault = tmp_path / "vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()

    # Note 1: knowledge note with frontmatter and wikilinks
    note1 = vault / "Project Ideas.md"
    note1.write_text(
        "---\ntags: [work, ideas]\n---\n\nSome thoughts about [[Alpha]] and [[Beta]].\n"
    )

    # Note 2: another knowledge note, same 5-min window
    note2 = vault / "Meeting Notes.md"
    note2.write_text("Notes from meeting with [[Charlie]].\n")

    # Note 3: daily note — still uses mtime for segment placement
    note3 = vault / "2026-03-01.md"
    note3.write_text("Daily log entry.\n")

    # Set mtimes: note1 and note2 within 5 min window, note3 in a different window
    import os

    base_ts = dt.datetime(2026, 3, 15, 10, 0, 0).timestamp()
    os.utime(note1, (base_ts, base_ts))
    os.utime(note2, (base_ts + 60, base_ts + 60))  # 1 min later, same window
    os.utime(note3, (base_ts + 600, base_ts + 600))  # 10 min later, different window

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = mod.ObsidianImporter().process(vault, tmp_path)

    assert result.entries_written == 3
    assert result.errors == []
    assert len(result.segments) == 2
    assert len(result.files_created) == 2

    # Both segments on same day (20260315) since all mtimes are on that day
    first_day, first_key = result.segments[0]
    second_day, second_key = result.segments[1]
    assert first_day == "20260315"
    assert second_day == "20260315"
    assert first_key == "100000_300"
    assert second_key == "101000_300"

    first_md = (
        day_path("20260315") / "import.obsidian" / "100000_300" / "note_transcript.md"
    )
    second_md = (
        day_path("20260315") / "import.obsidian" / "101000_300" / "note_transcript.md"
    )
    assert first_md.exists()
    assert second_md.exists()

    first_content = first_md.read_text()
    assert "## Project Ideas" in first_content
    assert "Tags: work, ideas" in first_content
    assert "[[Alpha]]" in first_content
    assert "[[Beta]]" in first_content
    assert "Some thoughts about" in first_content
    # Frontmatter should be stripped from content
    assert "---" not in first_content
    assert "## Meeting Notes" in first_content
    assert "[[Charlie]]" in first_content

    second_content = second_md.read_text()
    assert "## 2026-03-01" in second_content
    assert "Daily log entry." in second_content


def test_obsidian_render_note_markdown():
    """Test markdown rendering for a single note."""
    mod = importlib.import_module("solstone.think.importers.obsidian")

    note = {
        "title": "Test Note",
        "source_path": "subfolder/Test Note.md",
        "tags": ["project", "draft"],
        "wikilinks": ["Alice", "Project X"],
        "content": "---\ntags: [project, draft]\n---\n\nMain content here.\n",
    }

    rendered = mod._render_note_markdown(note)

    assert "## Test Note" in rendered
    assert "Source: subfolder/Test Note.md" in rendered
    assert "Tags: project, draft" in rendered
    assert "Links: [[Alice]], [[Project X]]" in rendered
    assert "Main content here." in rendered
    # Frontmatter stripped
    assert "---" not in rendered
