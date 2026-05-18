# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.importers.formatting — the import JSONL formatter."""

import json
import tempfile
from pathlib import Path

from solstone.think.importers.formatting import format_ai_chat, format_imported


def _make_entries(source: str, content_entries: list[dict]) -> list[dict]:
    """Build a complete JSONL entry list with header."""
    header = {
        "import": {"id": "20260101_120000", "source": source},
        "entry_count": len(content_entries),
    }
    return [header] + content_entries


def test_empty_entries():
    chunks, meta = format_imported([], None)
    assert chunks == []
    assert meta == {}


def test_header_only():
    entries = [{"import": {"id": "t", "source": "ics"}, "entry_count": 0}]
    chunks, meta = format_imported(entries, None)
    assert chunks == []
    assert meta["indexer"]["agent"] == "import.ics"


def test_calendar_event():
    entries = _make_entries(
        "ics",
        [
            {
                "type": "calendar_event",
                "ts": "2026-01-15T10:00:00",
                "title": "Team standup",
                "content": "Weekly sync",
                "duration_minutes": 30,
                "location": "Room 4B",
                "attendees": [
                    {"name": "Alice", "email": "alice@co.com"},
                    {"name": "Bob", "email": "bob@co.com"},
                ],
            }
        ],
    )
    chunks, meta = format_imported(entries, None)
    assert len(chunks) == 1
    assert meta["indexer"]["agent"] == "import.ics"

    md = chunks[0]["markdown"]
    assert "## Team standup" in md
    assert "30 min" in md
    assert "Room 4B" in md
    assert "Alice" in md
    assert "Bob" in md
    assert "Weekly sync" in md
    assert chunks[0]["timestamp"] > 0


def test_note():
    entries = _make_entries(
        "obsidian",
        [
            {
                "type": "note",
                "ts": "2026-01-15T00:00:00",
                "title": "Project ideas",
                "content": "Some thoughts on the new project.",
                "tags": ["work", "ideas"],
                "wikilinks": ["Project Alpha", "Bob"],
            }
        ],
    )
    chunks, meta = format_imported(entries, None)
    assert len(chunks) == 1
    assert meta["indexer"]["agent"] == "import.obsidian"

    md = chunks[0]["markdown"]
    assert "## Project ideas" in md
    assert "work, ideas" in md
    assert "Project Alpha" in md
    assert "Some thoughts" in md


def test_highlight():
    entries = _make_entries(
        "kindle",
        [
            {
                "type": "highlight",
                "ts": "2026-01-10T08:30:00",
                "book_title": "Thinking, Fast and Slow",
                "author": "Daniel Kahneman",
                "content": "Nothing in life is as important as you think it is.",
                "clip_type": "highlight",
                "page": 402,
                "location": "6120-6121",
            }
        ],
    )
    chunks, meta = format_imported(entries, None)
    assert len(chunks) == 1
    assert meta["indexer"]["agent"] == "import.kindle"

    md = chunks[0]["markdown"]
    assert "Thinking, Fast and Slow" in md
    assert "Daniel Kahneman" in md
    assert "Page 402" in md
    assert "> Nothing in life" in md


def test_highlight_note_type():
    entries = _make_entries(
        "kindle",
        [
            {
                "type": "highlight",
                "ts": "2026-01-10T08:30:00",
                "book_title": "Some Book",
                "author": "",
                "content": "My personal note",
                "clip_type": "note",
            }
        ],
    )
    chunks, meta = format_imported(entries, None)
    md = chunks[0]["markdown"]
    assert "Note: My personal note" in md


def test_generic_entry():
    entries = _make_entries(
        "custom",
        [
            {
                "type": "something_new",
                "ts": "2026-01-01T00:00:00",
                "title": "Mystery entry",
                "content": "Unknown format content.",
            }
        ],
    )
    chunks, meta = format_imported(entries, None)
    assert len(chunks) == 1
    md = chunks[0]["markdown"]
    assert "Mystery entry" in md
    assert "Unknown format content" in md


def test_multiple_entries():
    entries = _make_entries(
        "ics",
        [
            {
                "type": "calendar_event",
                "ts": "2026-01-15T09:00:00",
                "title": "Morning standup",
                "content": "",
            },
            {
                "type": "calendar_event",
                "ts": "2026-01-15T14:00:00",
                "title": "Design review",
                "content": "Review new mockups",
            },
        ],
    )
    chunks, meta = format_imported(entries, None)
    assert len(chunks) == 2
    assert "Morning standup" in chunks[0]["markdown"]
    assert "Design review" in chunks[1]["markdown"]
    # Timestamps should be ordered
    assert chunks[0]["timestamp"] < chunks[1]["timestamp"]


def test_formatter_registration():
    """Verify the formatter is registered and discoverable."""
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.ics/imported.jsonl")
    assert formatter is not None
    assert formatter.__name__ == "format_imported"


def test_formatter_registration_obsidian():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.obsidian/imported.jsonl")
    assert formatter is not None


def test_formatter_registration_ics_segment_markdown():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.ics/120000_300/imported.md")
    assert formatter is not None
    assert formatter.__name__ == "format_markdown"


def test_formatter_registration_kindle():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.kindle/103000_300/imported.md")
    assert formatter is not None
    assert formatter.__name__ == "format_markdown"


def test_formatter_registration_gemini_segment():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.gemini/100000_300/imported_audio.jsonl")
    assert formatter is not None
    assert formatter.__name__ == "format_ai_chat"


def test_formatter_registration_chatgpt_segment():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.chatgpt/100000_300/imported_audio.jsonl")
    assert formatter is not None
    assert formatter.__name__ == "format_ai_chat"


def test_formatter_registration_claude_segment():
    from solstone.think.formatters import get_formatter

    formatter = get_formatter("20260115/import.claude/100000_300/imported_audio.jsonl")
    assert formatter is not None
    assert formatter.__name__ == "format_ai_chat"


def test_path_metadata_extraction():
    """Verify day is correctly extracted from import paths."""
    from solstone.think.formatters import extract_path_metadata

    meta = extract_path_metadata("20260115/import.ics/imported.jsonl")
    assert meta["day"] == "20260115"
    assert meta["facet"] == ""

    meta = extract_path_metadata("20260301/import.obsidian/imported.jsonl")
    assert meta["day"] == "20260301"

    meta = extract_path_metadata("20260115/import.ics/120000_300/imported.md")
    assert meta["day"] == "20260115"
    assert meta["agent"] == "imported"


def test_find_formattable_includes_imports():
    """Verify find_formattable_files picks up import JSONL."""
    from solstone.think.formatters import find_formattable_files

    with tempfile.TemporaryDirectory() as tmpdir:
        # Create a fake import JSONL file
        import_dir = Path(tmpdir) / "20260115" / "import.ics"
        import_dir.mkdir(parents=True)
        jsonl_path = import_dir / "imported.jsonl"
        jsonl_path.write_text(
            json.dumps({"import": {"id": "t", "source": "ics"}, "entry_count": 0})
            + "\n"
        )

        files = find_formattable_files(tmpdir)
        assert "20260115/import.ics/imported.jsonl" in files


def test_find_formattable_includes_segment_markdown():
    from solstone.think.formatters import find_formattable_files

    with tempfile.TemporaryDirectory() as tmpdir:
        seg_dir = Path(tmpdir) / "20260115" / "import.ics" / "120000_300"
        seg_dir.mkdir(parents=True)
        md_path = seg_dir / "imported.md"
        md_path.write_text("## Test Event\n")

        files = find_formattable_files(tmpdir)
        assert "20260115/import.ics/120000_300/imported.md" in files


def test_format_ai_chat_empty():
    chunks, meta = format_ai_chat([], None)
    assert chunks == []
    assert meta == {}


def test_format_ai_chat_basic():
    entries = [
        {"imported": {"id": "20260101_120000", "facet": "work"}, "model": "gpt-4o"},
        {"start": "00:00:00", "speaker": "Human", "text": "Hello", "source": "import"},
        {
            "start": "00:00:05",
            "speaker": "Assistant",
            "text": "Hi there",
            "source": "import",
        },
    ]

    chunks, meta = format_ai_chat(
        entries,
        {
            "file_path": Path(
                "/journal/20260115/import.chatgpt/120000_300/imported_audio.jsonl"
            )
        },
    )

    assert len(chunks) == 2
    assert meta["indexer"]["agent"] == "import.chatgpt"
    assert meta["header"] == "# ChatGPT conversation\nModel: gpt-4o\nFacet: work"
    assert chunks[0]["markdown"] == "**Human:** Hello"
    assert chunks[1]["markdown"] == "**Assistant:** Hi there"
    assert chunks[1]["timestamp"] > chunks[0]["timestamp"]


def test_format_file_integration(monkeypatch):
    """End-to-end: write import JSONL, format it, get chunks."""
    from solstone.think.formatters import format_file

    with tempfile.TemporaryDirectory() as tmpdir:
        # Set SOLSTONE_JOURNAL for format_file
        monkeypatch.setenv("SOLSTONE_JOURNAL", tmpdir)

        import_dir = Path(tmpdir) / "20260115" / "import.ics"
        import_dir.mkdir(parents=True)
        jsonl_path = import_dir / "imported.jsonl"

        header = {"import": {"id": "t", "source": "ics"}, "entry_count": 1}
        entry = {
            "type": "calendar_event",
            "ts": "2026-01-15T10:00:00",
            "title": "Lunch with Alice",
            "content": "",
        }
        jsonl_path.write_text(json.dumps(header) + "\n" + json.dumps(entry) + "\n")

        chunks, meta = format_file(str(jsonl_path))
        assert len(chunks) == 1
        assert "Lunch with Alice" in chunks[0]["markdown"]
        assert meta["indexer"]["agent"] == "import.ics"
