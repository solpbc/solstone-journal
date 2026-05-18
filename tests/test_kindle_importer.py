# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.importers.kindle — Kindle My Clippings.txt importer."""

import os
import tempfile
from pathlib import Path

from solstone.think.entities.observations import load_observations
from solstone.think.importers.kindle import KindleImporter, _parse_block, _parse_date

importer = KindleImporter()


def _make_clipping(
    title: str = "Test Book (Author Name)",
    meta: str = "- Your Highlight on page 42 | location 100-101 | Added on Saturday, March 15, 2025 10:30:00 AM",
    content: str = "This is a highlighted passage.",
) -> str:
    return f"{title}\n{meta}\n\n{content}\n"


def _make_clippings_file(clippings: list[str]) -> str:
    return "==========\n".join(clippings) + "==========\n"


# --- Unit tests for helpers ---


def test_parse_date():
    result = _parse_date("Saturday, March 15, 2025 10:30:00 AM")
    assert result is not None
    assert result.month == 3
    assert result.day == 15


def test_parse_block_basic():
    block = _make_clipping()
    entry = _parse_block(block)
    assert entry is not None
    assert entry["type"] == "highlight"
    assert entry["book_title"] == "Test Book"
    assert entry["author"] == "Author Name"
    assert entry["content"] == "This is a highlighted passage."
    assert entry["page"] == 42
    assert entry["location"] == "100-101"


def test_parse_block_note():
    block = _make_clipping(
        meta="- Your Note on page 10 | Added on Saturday, March 15, 2025 10:30:00 AM",
        content="My personal note",
    )
    entry = _parse_block(block)
    assert entry is not None
    assert entry["clip_type"] == "note"


def test_parse_block_no_author():
    block = _make_clipping(title="Title Without Author")
    entry = _parse_block(block)
    assert entry is not None
    assert entry["book_title"] == "Title Without Author"
    assert entry["author"] == ""


# --- Detection tests ---


def test_detect_valid():
    content = _make_clippings_file([_make_clipping()])
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            assert importer.detect(Path(f.name)) is True
        finally:
            os.unlink(f.name)


def test_detect_wrong_format():
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write("Just some random text file.\n")
        f.flush()
        try:
            assert importer.detect(Path(f.name)) is False
        finally:
            os.unlink(f.name)


# --- Preview tests ---


def test_preview():
    content = _make_clippings_file(
        [
            _make_clipping(),
            _make_clipping(
                title="Another Book (Jane Doe)",
                meta="- Your Note on page 5 | Added on Sunday, March 16, 2025 02:00:00 PM",
                content="A note",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            preview = importer.preview(Path(f.name))
            assert preview.item_count == 2
            assert preview.entity_count > 0
            assert "2 books" in preview.summary
        finally:
            os.unlink(f.name)


# --- Process tests ---


def test_process_basic(monkeypatch):
    content = _make_clippings_file(
        [
            _make_clipping(),
            _make_clipping(
                meta="- Your Highlight on page 43 | Added on Saturday, March 15, 2025 10:31:00 AM",
                content="Another highlight from same session.",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            with tempfile.TemporaryDirectory() as journal:
                monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
                result = importer.process(Path(f.name), Path(journal))
                assert result.entries_written == 2
                assert result.errors == []
                assert result.segments is not None
                assert len(result.segments) >= 1

                md_path = Path(result.files_created[0])
                assert md_path.exists()
                assert md_path.name == "highlights_transcript.md"
                md = md_path.read_text()
                assert "Test Book" in md
                assert "> This is a highlighted passage." in md
                assert "Page 42" in md
        finally:
            os.unlink(f.name)


def test_process_multiple_windows(monkeypatch):
    """Highlights more than 5 minutes apart land in different segments."""
    content = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Highlight on page 1 | Added on Saturday, March 15, 2025 10:00:00 AM",
                content="First highlight",
            ),
            _make_clipping(
                meta="- Your Highlight on page 2 | Added on Saturday, March 15, 2025 10:10:00 AM",
                content="Second highlight, 10 min later",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            with tempfile.TemporaryDirectory() as journal:
                monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
                result = importer.process(Path(f.name), Path(journal))
                assert result.entries_written == 2
                assert result.segments is not None
                assert len(result.segments) == 2
                assert len(result.files_created) == 2
        finally:
            os.unlink(f.name)


def test_process_note_markdown(monkeypatch):
    """Notes render with Note: prefix instead of blockquote."""
    content = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Note on page 10 | Added on Saturday, March 15, 2025 10:30:00 AM",
                content="My personal note",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            with tempfile.TemporaryDirectory() as journal:
                monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
                result = importer.process(Path(f.name), Path(journal))
                md = Path(result.files_created[0]).read_text()
                assert "Note: My personal note" in md
        finally:
            os.unlink(f.name)


def test_observations_author_of(tmp_path, monkeypatch):
    content = _make_clippings_file([_make_clipping()])
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            author_obs = load_observations("test.kindle", "Author Name")
            author_contents = [o["content"] for o in author_obs]
            assert "Author of Test Book (via Kindle, 2025-03-15)" in author_contents
        finally:
            os.unlink(f.name)


def test_observations_by_author(tmp_path, monkeypatch):
    content = _make_clippings_file([_make_clipping()])
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Test Book")
            book_contents = [o["content"] for o in book_obs]
            assert "By Author Name (via Kindle, 2025-03-15)" in book_contents
        finally:
            os.unlink(f.name)


def test_observations_engagement(tmp_path, monkeypatch):
    content = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Highlight on page 42 | location 100-101 | Added on Saturday, March 15, 2025 10:30:00 AM",
            ),
            _make_clipping(
                meta="- Your Highlight on page 43 | location 102-103 | Added on Saturday, March 15, 2025 10:31:00 AM",
                content="Second highlight.",
            ),
            _make_clipping(
                meta="- Your Note on page 44 | location 104 | Added on Saturday, March 15, 2025 10:32:00 AM",
                content="A note.",
            ),
        ]
    )
    highlights_only = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Highlight on page 42 | location 100-101 | Added on Saturday, March 15, 2025 10:30:00 AM",
            ),
            _make_clipping(
                meta="- Your Highlight on page 43 | location 102-103 | Added on Saturday, March 15, 2025 10:31:00 AM",
                content="Second highlight.",
            ),
        ]
    )

    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Test Book")
            book_contents = [o["content"] for o in book_obs]
            assert "2 highlights, 1 notes (via Kindle, 2025-03-15)" in book_contents
        finally:
            os.unlink(f.name)

    second_tmp_path = tmp_path / "highlights_only"
    second_tmp_path.mkdir()
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(highlights_only)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(second_tmp_path))
            importer.process(Path(f.name), second_tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Test Book")
            book_contents = [o["content"] for o in book_obs]
            assert "2 highlights (via Kindle, 2025-03-15)" in book_contents
        finally:
            os.unlink(f.name)


def test_observations_multi_book_author(tmp_path, monkeypatch):
    content = _make_clippings_file(
        [
            _make_clipping(title="Test Book (Author Name)"),
            _make_clipping(title="Second Book (Author Name)"),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            author_obs = load_observations("test.kindle", "Author Name")
            author_contents = [o["content"] for o in author_obs]
            assert "Author of Test Book (via Kindle, 2025-03-15)" in author_contents
            assert "Author of Second Book (via Kindle, 2025-03-15)" in author_contents
        finally:
            os.unlink(f.name)


def test_observations_no_author(tmp_path, monkeypatch):
    content = _make_clippings_file([_make_clipping(title="Title Without Author")])
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Title Without Author")
            book_contents = [o["content"] for o in book_obs]
            assert not any(c.startswith("By ") for c in book_contents)
            author_entities_dir = tmp_path / "facets" / "test.kindle" / "entities"
            if author_entities_dir.exists():
                entity_names = {
                    entity_dir.name
                    for entity_dir in author_entities_dir.iterdir()
                    if entity_dir.is_dir()
                }
                assert "author_name" not in entity_names
        finally:
            os.unlink(f.name)


def test_observations_dedup_on_reimport(tmp_path, monkeypatch):
    content = _make_clippings_file([_make_clipping()])
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            first = load_observations("test.kindle", "Test Book")
            first_by_author = [
                o
                for o in first
                if o["content"] == "By Author Name (via Kindle, 2025-03-15)"
            ]
            assert len(first_by_author) == 1

            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            second = load_observations("test.kindle", "Test Book")
            second_by_author = [
                o
                for o in second
                if o["content"] == "By Author Name (via Kindle, 2025-03-15)"
            ]
            assert len(second_by_author) == 1
        finally:
            os.unlink(f.name)


def test_observations_engagement_notes_only(tmp_path, monkeypatch):
    """Notes-only book gets notes count without highlights."""
    content = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Note on page 10 | Added on Saturday, March 15, 2025 10:30:00 AM",
                content="A note.",
            ),
            _make_clipping(
                meta="- Your Note on page 11 | Added on Saturday, March 15, 2025 10:31:00 AM",
                content="Another note.",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Test Book")
            book_contents = [o["content"] for o in book_obs]
            assert "2 notes (via Kindle, 2025-03-15)" in book_contents
            # No highlights count should appear
            assert not any("highlights" in c for c in book_contents)
        finally:
            os.unlink(f.name)


def test_observations_engagement_excludes_bookmarks(tmp_path, monkeypatch):
    content = _make_clippings_file(
        [
            _make_clipping(
                meta="- Your Highlight on page 42 | location 100-101 | Added on Saturday, March 15, 2025 10:30:00 AM",
            ),
            _make_clipping(
                meta="- Your Bookmark on page 43 | location 102-103 | Added on Saturday, March 15, 2025 10:31:00 AM",
                content="Saved bookmark.",
            ),
        ]
    )
    with tempfile.NamedTemporaryFile(suffix=".txt", mode="w", delete=False) as f:
        f.write(content)
        f.flush()
        try:
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
            importer.process(Path(f.name), tmp_path, facet="test.kindle")
            book_obs = load_observations("test.kindle", "Test Book")
            book_contents = [o["content"] for o in book_obs]
            assert "1 highlights (via Kindle, 2025-03-15)" in book_contents
        finally:
            os.unlink(f.name)


# --- Registry test ---


def test_registered_in_registry():
    from solstone.think.importers.file_importer import (
        FILE_IMPORTER_REGISTRY,
        get_file_importer,
    )

    assert "kindle" in FILE_IMPORTER_REGISTRY
    imp = get_file_importer("kindle")
    assert imp is not None
    assert imp.name == "kindle"
