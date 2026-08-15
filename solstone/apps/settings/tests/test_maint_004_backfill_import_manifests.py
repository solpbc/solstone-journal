# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
import os
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from solstone.think.importers.shared import (
    find_manifest_by_hash,
    hash_source,
    write_manifest,
)

_mod = importlib.import_module(
    "solstone.apps.settings.maint.004_backfill_import_manifests"
)
backfill = _mod.backfill


@pytest.fixture(autouse=True)
def _set_journal(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))


def _make_import(
    journal,
    ts,
    *,
    on_disk_name,
    file_path,
    original_filename=None,
    imported=None,
    write_media=True,
):
    import_dir = journal / "imports" / ts
    import_dir.mkdir(parents=True, exist_ok=True)
    if write_media:
        (import_dir / on_disk_name).write_bytes(f"{ts}:{on_disk_name}".encode())
    (import_dir / "import.json").write_text(
        json.dumps(
            {
                "original_filename": original_filename or on_disk_name,
                "file_path": file_path,
                "upload_timestamp": 0,
                "file_size": 0,
            }
        ),
        encoding="utf-8",
    )
    if imported is not None:
        (import_dir / "imported.json").write_text(
            json.dumps(imported), encoding="utf-8"
        )
    return import_dir


def test_backfills_retained_original(tmp_path):
    journal = tmp_path
    ts = "20250630_143256"
    import_dir = _make_import(
        journal,
        ts,
        on_disk_name="recording.m4a",
        file_path="/stale/imports/20250630_143256/recording.m4a",
        imported={
            "source_type": "apple",
            "target_day": "20250630",
            "all_created_files": ["a", "b"],
        },
    )
    retained = import_dir / "recording.m4a"

    counts = backfill(journal)

    manifest_path = import_dir / "manifest.json"
    assert manifest_path.exists()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["source_hash"] == hash_source(retained)
    assert manifest["import_id"] == ts
    assert manifest["source_type"] == "apple"
    assert manifest["days_affected"] == ["20250630"]
    assert manifest["entry_count"] == 2
    assert manifest["files_created"] == ["a", "b"]
    assert counts == {
        "scanned": 1,
        "backfilled": 1,
        "skipped_already_has_manifest": 0,
        "skipped_no_retained_original": 0,
    }


def test_e2e_real_import_byte_hash_match(tmp_path, monkeypatch):
    cli = importlib.import_module("solstone.think.importers.cli")
    text_mod = importlib.import_module("solstone.think.importers.text")

    known_bytes = b"hello\nworld"
    src = tmp_path / "My Notes.txt"
    src.write_bytes(known_bytes)

    monkeypatch.setattr(
        cli, "detect_created", lambda p, **kw: {"day": "20240101", "time": "120000"}
    )

    def mock_detect_segment(text, start_time):
        return [("12:00:00", "seg1"), ("12:05:00", "seg2")]

    monkeypatch.setattr(text_mod, "detect_transcript_segment", mock_detect_segment)

    def mock_detect_json(text, segment_start):
        return {
            "entries": [{"start": segment_start, "speaker": "Unknown", "text": text}],
            "topics": "",
            "setting": "",
        }

    monkeypatch.setattr(text_mod, "detect_transcript_json", mock_detect_json)
    monkeypatch.setattr(cli, "CallosumConnection", lambda **kwargs: MagicMock())
    monkeypatch.setattr(cli, "_status_emitter", lambda: None)
    monkeypatch.setattr(
        "sys.argv",
        ["sol import", str(src), "--timestamp", "20240101_120000"],
    )
    cli.main()

    journal = tmp_path
    import_dir = journal / "imports" / "20240101_120000"
    import_meta = json.loads((import_dir / "import.json").read_text(encoding="utf-8"))
    retained = import_dir / os.path.basename(import_meta["file_path"])
    assert retained.is_file()

    manifest_path = import_dir / "manifest.json"
    assert manifest_path.exists()
    manifest_path.unlink()
    assert not manifest_path.exists()

    counts = backfill(journal)

    assert counts["backfilled"] == 1
    assert find_manifest_by_hash(journal, hash_source(src)) is not None


def test_locator_uses_basename_not_original_filename(tmp_path):
    journal = tmp_path
    ts = "20250630_143256"
    original_filename = "Method Coffee Roasters.m4a"
    file_path = (
        "/workspace/owner/Pictures/Eri/imports/20250630_143256/Method_Coffee_Roasters.m4a"
    )
    import_dir = _make_import(
        journal,
        ts,
        on_disk_name="Method_Coffee_Roasters.m4a",
        file_path=file_path,
        original_filename=original_filename,
    )

    counts = backfill(journal)

    manifest = json.loads((import_dir / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["source_hash"] == hash_source(
        import_dir / "Method_Coffee_Roasters.m4a"
    )
    assert not Path(file_path).is_file()
    assert not (import_dir / original_filename).is_file()
    assert counts["backfilled"] == 1


def test_idempotent_existing_manifest_untouched(tmp_path):
    journal = tmp_path
    ts = "20250630_143256"
    import_dir = _make_import(
        journal,
        ts,
        on_disk_name="recording.mp3",
        file_path="/stale/imports/20250630_143256/recording.mp3",
    )
    manifest_path = write_manifest(
        journal,
        import_id=ts,
        source_type="audio",
        source_hash="SENTINEL",
        entry_count=0,
        files_created=[],
        days_affected=[],
    )
    before = manifest_path.read_bytes()

    counts = backfill(journal)

    assert (import_dir / "manifest.json").read_bytes() == before
    assert counts["skipped_already_has_manifest"] == 1
    assert counts["backfilled"] == 0


def test_no_retained_original_skipped_not_fabricated(tmp_path):
    journal = tmp_path
    ts = "20250630_143256"
    import_dir = _make_import(
        journal,
        ts,
        on_disk_name="missing.m4a",
        file_path="/stale/imports/20250630_143256/missing.m4a",
        imported={"all_created_files": ["segment-one.jsonl"]},
        write_media=False,
    )

    counts = backfill(journal)

    assert not (import_dir / "manifest.json").exists()
    assert counts["skipped_no_retained_original"] == 1
    assert counts["backfilled"] == 0


def test_second_run_is_noop(tmp_path):
    journal = tmp_path
    _make_import(
        journal,
        "20250630_143256",
        on_disk_name="first.txt",
        file_path="/stale/imports/20250630_143256/first.txt",
    )
    _make_import(
        journal,
        "20250630_143257",
        on_disk_name="second.m4a",
        file_path="/stale/imports/20250630_143257/second.m4a",
    )

    first = backfill(journal)
    second = backfill(journal)

    assert first["backfilled"] == 2
    assert second["backfilled"] == 0
    assert second["skipped_already_has_manifest"] == 2
    assert (
        second["scanned"]
        == second["backfilled"]
        + second["skipped_already_has_manifest"]
        + second["skipped_no_retained_original"]
    )
