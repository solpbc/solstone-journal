# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for import deduplication — manifests, source-level dedup, entry merge."""

import hashlib
import json
import os
import tempfile
from pathlib import Path

from solstone.think.importers.shared import (
    _build_import_manifest,
    _entry_content_key,
    _load_existing_entries,
    find_manifest_by_hash,
    hash_source,
    write_manifest,
    write_structured_import,
)

# --- hash_source tests ---


def test_hash_source_file():
    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
        f.write('{"test": true}')
        f.flush()
        try:
            h1 = hash_source(Path(f.name))
            h2 = hash_source(Path(f.name))
            assert h1 == h2
            assert len(h1) == 64  # SHA-256 hex
        finally:
            os.unlink(f.name)


def test_hash_source_file_different_content():
    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f1:
        f1.write('{"a": 1}')
        f1.flush()
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f2:
            f2.write('{"a": 2}')
            f2.flush()
            try:
                assert hash_source(Path(f1.name)) != hash_source(Path(f2.name))
            finally:
                os.unlink(f1.name)
                os.unlink(f2.name)


def test_hash_source_directory():
    with tempfile.TemporaryDirectory() as tmpdir:
        (Path(tmpdir) / "a.txt").write_text("hello")
        (Path(tmpdir) / "b.txt").write_text("world")
        h1 = hash_source(Path(tmpdir))
        h2 = hash_source(Path(tmpdir))
        assert h1 == h2


def test_hash_source_directory_changes():
    with tempfile.TemporaryDirectory() as tmpdir:
        (Path(tmpdir) / "a.txt").write_text("hello")
        h1 = hash_source(Path(tmpdir))
        (Path(tmpdir) / "b.txt").write_text("world")
        h2 = hash_source(Path(tmpdir))
        assert h1 != h2


# --- manifest tests ---


def test_build_import_manifest():
    with tempfile.TemporaryDirectory() as tmpdir:
        import_dir = Path(tmpdir)
        alpha = import_dir / "alpha.txt"
        alpha_bytes = b"alpha payload"
        alpha.write_bytes(alpha_bytes)
        beta = import_dir / "nested" / "beta.bin"
        beta.parent.mkdir(parents=True)
        beta_bytes = b"\x00\x01beta"
        beta.write_bytes(beta_bytes)

        manifest = _build_import_manifest(import_dir)

        assert manifest["import_dir"] == str(import_dir)
        assert manifest["file_count"] == 2
        assert manifest["total_bytes"] == len(alpha_bytes) + len(beta_bytes)
        assert manifest["files"] == [
            {
                "name": "alpha.txt",
                "bytes": len(alpha_bytes),
                "hash": hashlib.sha256(alpha_bytes).hexdigest(),
            },
            {
                "name": "nested/beta.bin",
                "bytes": len(beta_bytes),
                "hash": hashlib.sha256(beta_bytes).hexdigest(),
            },
        ]
        assert manifest["timestamp"].endswith("+00:00")


def test_write_and_find_manifest():
    with tempfile.TemporaryDirectory() as journal:
        manifest_path = write_manifest(
            Path(journal),
            import_id="20260115_120000",
            source_type="ics",
            source_hash="abc123",
            entry_count=42,
            files_created=[
                f"{journal}/20260115/import.ics/imported.jsonl",
                f"{journal}/20260116/import.ics/imported.jsonl",
            ],
        )
        assert manifest_path.exists()

        # Read it back
        with open(manifest_path) as f:
            data = json.load(f)
        assert data["source_type"] == "ics"
        assert data["source_hash"] == "abc123"
        assert data["entry_count"] == 42
        assert "20260115" in data["days_affected"]
        assert "20260116" in data["days_affected"]

        # Find by hash
        found = find_manifest_by_hash(Path(journal), "abc123")
        assert found is not None
        assert found["source_type"] == "ics"

        # Not found for different hash
        assert find_manifest_by_hash(Path(journal), "xyz999") is None


def test_find_manifest_no_imports_dir():
    with tempfile.TemporaryDirectory() as journal:
        assert find_manifest_by_hash(Path(journal), "abc") is None


def test_find_manifest_bad_json():
    with tempfile.TemporaryDirectory() as journal:
        manifest_dir = Path(journal) / "imports" / "bad"
        manifest_dir.mkdir(parents=True)
        (manifest_dir / "manifest.json").write_text("not json")
        assert find_manifest_by_hash(Path(journal), "abc") is None


# --- entry content key tests ---


def test_entry_content_key():
    e1 = {"type": "calendar_event", "ts": "2026-01-15T10:00:00", "title": "Standup"}
    e2 = {"type": "calendar_event", "ts": "2026-01-15T10:00:00", "title": "Standup"}
    e3 = {"type": "calendar_event", "ts": "2026-01-15T11:00:00", "title": "Review"}
    assert _entry_content_key(e1) == _entry_content_key(e2)
    assert _entry_content_key(e1) != _entry_content_key(e3)


def test_entry_content_key_kindle():
    e = {"type": "highlight", "ts": "2026-01-10T08:00:00", "book_title": "Deep Work"}
    key = _entry_content_key(e)
    assert "Deep Work" in key


# --- entry-level merge in write_structured_import ---


def test_reimport_same_entries_no_duplicates(monkeypatch):
    """Re-importing identical entries should not create duplicates."""
    with tempfile.TemporaryDirectory() as journal:
        monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
        entries = [
            {
                "type": "calendar_event",
                "ts": "2026-01-15T10:00:00",
                "title": "Standup",
                "content": "Daily sync",
            },
            {
                "type": "calendar_event",
                "ts": "2026-01-15T14:00:00",
                "title": "Review",
                "content": "Code review",
            },
        ]

        # First import
        files1 = write_structured_import("ics", entries, import_id="t1")
        assert len(files1) == 1

        # Read entry count
        lines1 = Path(files1[0]).read_text().strip().split("\n")
        header1 = json.loads(lines1[0])
        assert header1["entry_count"] == 2

        # Re-import same entries
        files2 = write_structured_import("ics", entries, import_id="t2")
        assert len(files2) == 1

        # Entry count should still be 2 (no duplicates)
        lines2 = Path(files2[0]).read_text().strip().split("\n")
        header2 = json.loads(lines2[0])
        assert header2["entry_count"] == 2


def test_reimport_with_new_entries_merges(monkeypatch):
    """Re-importing with new entries should merge (add new, keep old)."""
    with tempfile.TemporaryDirectory() as journal:
        monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
        original = [
            {
                "type": "calendar_event",
                "ts": "2026-01-15T10:00:00",
                "title": "Standup",
                "content": "Daily sync",
            },
        ]
        updated = original + [
            {
                "type": "calendar_event",
                "ts": "2026-01-15T14:00:00",
                "title": "New meeting",
                "content": "Added later",
            },
        ]

        # First import
        write_structured_import("ics", original, import_id="t1")

        # Import with new entries
        files = write_structured_import("ics", updated, import_id="t2")

        # Should have both entries
        lines = Path(files[0]).read_text().strip().split("\n")
        header = json.loads(lines[0])
        assert header["entry_count"] == 2

        # Verify content
        entries = [json.loads(line) for line in lines[1:]]
        titles = [e["title"] for e in entries]
        assert "Standup" in titles
        assert "New meeting" in titles


def test_first_import_no_merge_needed(monkeypatch):
    """First import should work normally with no existing file."""
    with tempfile.TemporaryDirectory() as journal:
        monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
        entries = [
            {
                "type": "note",
                "ts": "2026-02-01T00:00:00",
                "title": "Test note",
                "content": "Hello",
            },
        ]
        files = write_structured_import("obsidian", entries, import_id="t1")
        assert len(files) == 1
        assert Path(files[0]).exists()


def test_load_existing_entries():
    with tempfile.TemporaryDirectory() as tmpdir:
        p = Path(tmpdir) / "imported.jsonl"
        header = {"import": {"id": "t", "source": "ics"}, "entry_count": 1}
        entry = {"type": "calendar_event", "ts": "2026-01-15T10:00:00", "title": "X"}
        p.write_text(json.dumps(header) + "\n" + json.dumps(entry) + "\n")

        entries = _load_existing_entries(p)
        assert len(entries) == 1
        assert entries[0]["title"] == "X"


def test_load_existing_entries_missing_file():
    entries = _load_existing_entries(Path("/nonexistent/path.jsonl"))
    assert entries == []
