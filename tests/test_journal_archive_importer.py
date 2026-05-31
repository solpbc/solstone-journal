# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import datetime as dt
import importlib
import json
import os
import shutil
import threading
import zipfile
from pathlib import Path
from unittest.mock import MagicMock

import pytest

import solstone.think.importers.journal_archive as journal_archive
from solstone.think.importers.file_importer import ImportResult


def _reset_journal(monkeypatch, journal_root: Path) -> None:
    journal_root.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_root))
    think_utils = importlib.import_module("solstone.think.utils")
    think_utils._journal_path_cache = None


def _write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def _build_archive_members(
    *,
    prefix: str = "",
    day: str = "20260101",
    source_entity_id: str = "source_person",
    source_name: str = "Source Person",
    source_is_principal: bool = False,
) -> dict[str, str]:
    entity_payload = {
        "id": source_entity_id,
        "name": source_name,
        "type": "person",
        "created_at": 1,
        "is_principal": source_is_principal,
    }
    return {
        f"{prefix}chronicle/{day}/default/090000_300/audio.jsonl": "{}\n",
        f"{prefix}entities/{source_entity_id}/entity.json": json.dumps(entity_payload),
        f"{prefix}facets/work/facet.json": json.dumps({"title": "Work"}),
        f"{prefix}imports/{day}_090000/manifest.json": "{}\n",
        f"{prefix}_export.json": json.dumps(
            {
                "solstone_version": "0.1.0",
                "exported_at": "2026-04-26T20:00:00Z",
                "source_journal": "/tmp/source",
                "day_count": 1,
                "entity_count": 1,
                "facet_count": 1,
            }
        ),
    }


def _write_archive(path: Path, members: dict[str, str]) -> None:
    with zipfile.ZipFile(path, "w") as archive:
        for name, payload in members.items():
            archive.writestr(name, payload)


def _hold_merge_lock(
    journal_root: Path,
    kind: str,
    import_id: str,
    ready: threading.Event,
    release: threading.Event,
) -> None:
    with journal_archive.acquire_merge_lock(journal_root, kind, import_id):
        ready.set()
        release.wait(timeout=5)


def test_journal_archive_importer_detect_accepts_valid_export_zip(tmp_path):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())

    assert journal_archive.JournalArchiveImporter().detect(archive_path) is True


def test_journal_archive_importer_preview_uses_validator_counts(tmp_path):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())

    preview = journal_archive.JournalArchiveImporter().preview(archive_path)

    assert preview.date_range == ("20260101", "20260101")
    assert preview.item_count == 1
    assert preview.entity_count == 1
    assert "1 days" in preview.summary


def test_journal_archive_importer_process_merges_wrapped_archive(tmp_path, monkeypatch):
    archive_path = tmp_path / "wrapped-export.zip"
    _write_archive(archive_path, _build_archive_members(prefix="snapshot/"))
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)

    send = MagicMock(return_value=True)
    monkeypatch.setattr(journal_archive, "callosum_send", send)

    result = journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
    )

    assert result.errors == []
    assert result.merge_summary is not None
    assert result.merge_summary["segments_copied"] == 1
    assert result.merge_log_path is not None
    assert result.merge_staging_path is not None
    assert (target / "chronicle" / "20260101" / "default" / "090000_300").exists()
    assert (target / "entities" / "source_person" / "entity.json").exists()
    assert (target / "imports" / "20260101_090000" / "manifest.json").exists()
    send.assert_called_once_with(
        "supervisor",
        "request",
        cmd=["journal", "indexer", "--rescan-full"],
    )


def test_dispatcher_blocks_file_import_when_merge_lock_held(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.cli")
    ics_file = tmp_path / "calendar.ics"
    ics_file.write_text("BEGIN:VCALENDAR\nEND:VCALENDAR", encoding="utf-8")

    _reset_journal(monkeypatch, tmp_path)
    ready = threading.Event()
    release = threading.Event()
    holder = threading.Thread(
        target=_hold_merge_lock,
        args=(tmp_path, "file-import", "lock-holder", ready, release),
        daemon=True,
    )
    holder.start()
    assert ready.wait(timeout=5)

    mock_imp = MagicMock()
    mock_imp.name = "ics"
    mock_imp.display_name = "ICS Calendar"
    callosum = MagicMock()

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

    with pytest.raises(SystemExit, match="pid"):
        mod.main()

    assert mock_imp.process.call_count == 0
    assert (tmp_path / "imports" / "20260303_120000" / "imported.json").exists()

    release.set()
    holder.join(timeout=5)


def test_dispatcher_treats_archive_lock_contention_as_failure(tmp_path, monkeypatch):
    mod = importlib.import_module("solstone.think.importers.cli")
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    _reset_journal(monkeypatch, tmp_path)
    ready = threading.Event()
    release = threading.Event()
    holder = threading.Thread(
        target=_hold_merge_lock,
        args=(tmp_path, "journal-archive-import", "lock-holder", ready, release),
        daemon=True,
    )
    holder.start()
    assert ready.wait(timeout=5)

    callosum = MagicMock()
    monkeypatch.setattr(
        "sys.argv",
        [
            "sol import",
            str(archive_path),
            "--source",
            "journal_archive",
            "--timestamp",
            "20260303_120000",
        ],
    )
    monkeypatch.setattr(mod, "CallosumConnection", lambda **kwargs: callosum)
    monkeypatch.setattr(mod, "get_rev", lambda: "test-rev")
    monkeypatch.setattr(mod, "_status_emitter", lambda: None)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=True))

    with pytest.raises(SystemExit, match="pid"):
        mod.main()

    emit_kinds = [call.args[:2] for call in callosum.emit.call_args_list]
    assert ("importer", "file_imported") not in emit_kinds
    assert ("importer", "error") in emit_kinds

    imported_path = tmp_path / "imports" / "20260303_120000" / "imported.json"
    payload = json.loads(imported_path.read_text(encoding="utf-8"))
    assert "processing_failed" in payload
    assert payload["error_stage"] == "importing"

    release.set()
    holder.join(timeout=5)


def test_journal_archive_importer_process_blocks_when_lock_held(tmp_path, monkeypatch):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)

    ready = threading.Event()
    release = threading.Event()
    holder = threading.Thread(
        target=_hold_merge_lock,
        args=(target, "journal-archive-import", "lock-holder", ready, release),
        daemon=True,
    )
    holder.start()
    assert ready.wait(timeout=5)

    with pytest.raises(journal_archive.MergeLockError, match="pid"):
        journal_archive.JournalArchiveImporter().process(
            archive_path,
            target,
            import_id="20260426_120000",
        )

    assert not (target / "chronicle").exists()

    release.set()
    holder.join(timeout=5)


def test_journal_archive_importer_process_raises_on_invalid_archive(tmp_path):
    archive_path = tmp_path / "invalid.zip"
    archive_path.write_text("not a zip", encoding="utf-8")

    with pytest.raises(ValueError, match="readable ZIP file"):
        journal_archive.JournalArchiveImporter().process(
            archive_path,
            tmp_path / "target",
            import_id="20260426_120000",
        )


def test_journal_archive_importer_process_bridges_merge_progress(tmp_path, monkeypatch):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=True))

    events: list[tuple[int, int, str | None]] = []

    def progress_callback(current, total, **kwargs):
        events.append((current, total, kwargs.get("stage")))

    journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
        progress_callback=progress_callback,
    )

    stages = {stage for _, _, stage in events}
    assert {"segments", "entities", "facets", "imports"} <= stages


def test_journal_archive_importer_process_reports_principal_collision(
    tmp_path, monkeypatch
):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(
        archive_path,
        _build_archive_members(
            source_entity_id="source_principal",
            source_name="Source Principal",
            source_is_principal=True,
        ),
    )
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    _write_json(
        target / "entities" / "target_principal" / "entity.json",
        {
            "id": "target_principal",
            "name": "Target Principal",
            "type": "person",
            "created_at": 1,
            "is_principal": True,
        },
    )

    result = journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
        dry_run=True,
    )

    assert result.principal_collision == {
        "source_entity_id": "source_principal",
        "source_name": "Source Principal",
        "target_entity_id": "target_principal",
        "target_name": "Target Principal",
    }


def test_journal_archive_importer_logs_segment_errors(tmp_path, monkeypatch):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=True))

    merge_mod = importlib.import_module("solstone.think.merge")
    original_copytree = merge_mod.shutil.copytree

    def failing_copytree(src, dst, *args, **kwargs):
        if Path(src).name == "090000_300":
            raise OSError("segment copy failed")
        return original_copytree(src, dst, *args, **kwargs)

    monkeypatch.setattr(merge_mod.shutil, "copytree", failing_copytree)

    result = journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
    )

    assert result.merge_summary is not None
    assert result.merge_summary["segments_errored"] == 1
    assert result.merge_log_path is not None
    decision_log = Path(result.merge_log_path)
    rows = [
        json.loads(line)
        for line in decision_log.read_text(encoding="utf-8").splitlines()
        if line
    ]
    assert {
        "action": "segment_errored",
        "item_type": "segment",
        "item_id": "20260101/default/090000_300",
        "reason": "segment copy failed",
    }.items() <= next(
        row.items() for row in rows if row.get("action") == "segment_errored"
    )


def test_journal_archive_importer_logs_staged_entity_paths(tmp_path, monkeypatch):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(
        archive_path,
        _build_archive_members(
            source_entity_id="shared_person",
            source_name="Source Person",
        ),
    )
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=True))
    _write_json(
        target / "entities" / "shared_person" / "entity.json",
        {
            "id": "shared_person",
            "name": "Target Person",
            "type": "person",
            "created_at": 1,
        },
    )

    result = journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
    )

    assert result.merge_summary is not None
    assert result.merge_summary["entities_staged"] == 1
    assert result.merge_log_path is not None
    assert result.merge_staging_path is not None
    decision_log = Path(result.merge_log_path)
    rows = [
        json.loads(line)
        for line in decision_log.read_text(encoding="utf-8").splitlines()
        if line
    ]
    staged_row = next(row for row in rows if row.get("action") == "entity_staged")
    expected_path = str(
        Path(result.merge_staging_path) / "shared_person" / "entity.json"
    )
    assert staged_row["staging_path"] == expected_path
    assert staged_row["staging_path"].startswith(result.merge_staging_path)


def test_journal_archive_importer_process_dry_run_is_read_only(tmp_path, monkeypatch):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    send = MagicMock(return_value=True)
    monkeypatch.setattr(journal_archive, "callosum_send", send)

    result = journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
        dry_run=True,
    )

    assert result.merge_summary is not None
    assert result.merge_summary["segments_copied"] == 1
    assert not (target / "chronicle").exists()
    send.assert_not_called()


def test_journal_archive_importer_safe_extract_rejects_escape_target(
    tmp_path, monkeypatch
):
    archive_path = tmp_path / "unsafe.zip"
    _write_archive(
        archive_path,
        {
            "chronicle/20260101/default/090000_300/audio.jsonl": "{}\n",
            "../escape.txt": "unsafe\n",
        },
    )
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    importer = journal_archive.JournalArchiveImporter()
    validation = journal_archive.ArchiveValidation(
        ok=True,
        archive_path=archive_path,
        root_prefix="",
        manifest=None,
    )

    with pytest.raises(ImportError, match="unsafe path"):
        importer._safe_extract(archive_path, validation, "20260426_120000")


def test_journal_archive_importer_safe_extract_skips_metadata_entries(
    tmp_path, monkeypatch
):
    archive_path = tmp_path / "metadata.zip"
    _write_archive(
        archive_path,
        {
            "__MACOSX/ignored.txt": "ignored\n",
            "chronicle/20260101/default/090000_300/audio.jsonl": "{}\n",
            ".DS_Store": "ignored\n",
            "_export.json": json.dumps(
                {
                    "solstone_version": "0.1.0",
                    "exported_at": "2026-04-26T20:00:00Z",
                    "source_journal": "/tmp/source",
                    "day_count": 1,
                    "entity_count": 0,
                    "facet_count": 0,
                }
            ),
        },
    )
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    validation = journal_archive.validate_journal_archive(archive_path)
    importer = journal_archive.JournalArchiveImporter()
    extracted_root, temp_dir = importer._safe_extract(
        archive_path,
        validation,
        "20260426_120000",
    )
    try:
        assert (
            extracted_root / "chronicle" / "20260101" / "default" / "090000_300"
        ).exists()
        assert not (extracted_root / "__MACOSX").exists()
        assert not (extracted_root / ".DS_Store").exists()
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def test_journal_archive_importer_process_requests_async_full_rescan(
    tmp_path, monkeypatch
):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    send = MagicMock(return_value=True)
    monkeypatch.setattr(journal_archive, "callosum_send", send)

    journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
    )

    send.assert_called_once_with(
        "supervisor",
        "request",
        cmd=["journal", "indexer", "--rescan-full"],
    )


def test_journal_archive_importer_warns_when_callosum_send_fails(
    tmp_path, monkeypatch, caplog
):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=False))

    with caplog.at_level("WARNING"):
        result = journal_archive.JournalArchiveImporter().process(
            archive_path,
            target,
            import_id="20260426_120000",
        )

    assert result.errors == []
    assert "post-merge full reindex: callosum_send returned false" in caplog.text


def test_journal_archive_importer_process_cleans_extract_dir_on_success_and_error(
    tmp_path, monkeypatch
):
    archive_path = tmp_path / "journal-export.zip"
    _write_archive(archive_path, _build_archive_members())
    extract_root = tmp_path / "extracts"
    extract_root.mkdir()
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", extract_root)

    target = tmp_path / "target"
    _reset_journal(monkeypatch, target)
    monkeypatch.setattr(journal_archive, "callosum_send", MagicMock(return_value=True))

    journal_archive.JournalArchiveImporter().process(
        archive_path,
        target,
        import_id="20260426_120000",
    )
    assert list(extract_root.glob("solstone-merge-*")) == []

    def boom(*args, **kwargs):
        raise RuntimeError("boom")

    monkeypatch.setattr(journal_archive, "merge_journals", boom)
    with pytest.raises(RuntimeError, match="boom"):
        journal_archive.JournalArchiveImporter().process(
            archive_path,
            target,
            import_id="20260426_120001",
        )
    assert list(extract_root.glob("solstone-merge-*")) == []


def test_sweep_stale_extract_dirs_removes_old_directories(tmp_path, monkeypatch):
    monkeypatch.setattr(journal_archive, "TEMP_EXTRACT_ROOT", tmp_path)
    stale = tmp_path / "solstone-merge-stale"
    stale.mkdir()
    fresh = tmp_path / "solstone-merge-fresh"
    fresh.mkdir()

    old_ts = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=2)).timestamp()
    os.utime(stale, (old_ts, old_ts))

    swept = journal_archive.sweep_stale_extract_dirs()

    assert swept == 1
    assert not stale.exists()
    assert fresh.exists()


def test_importer_cli_emits_merge_summary_and_principal_collision(
    tmp_path, monkeypatch, capsys
):
    mod = importlib.import_module("solstone.think.importers.cli")
    archive_path = tmp_path / "journal-export.zip"
    archive_path.write_bytes(b"fake zip")

    _reset_journal(monkeypatch, tmp_path)
    callosum = MagicMock()
    mock_imp = MagicMock()
    mock_imp.name = "journal_archive"
    mock_imp.display_name = "Journal Archive"
    mock_imp.process.return_value = ImportResult(
        entries_written=1,
        entities_seeded=0,
        files_created=[],
        errors=["segment 20260101/default/090000_300: segment copy failed"],
        summary="Merged archive",
        merge_summary={"segments_copied": 1},
        principal_collision={"source_entity_id": "a"},
        merge_log_path="/tmp/journal.merge/run/decisions.jsonl",
        merge_staging_path="/tmp/journal.merge/run/staging",
    )

    monkeypatch.setattr(
        "sys.argv",
        [
            "sol import",
            str(archive_path),
            "--source",
            "journal_archive",
            "--timestamp",
            "20260303_120000",
            "--json",
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

    file_imported = next(
        call
        for call in callosum.emit.call_args_list
        if call.args[:2] == ("importer", "file_imported")
    )
    assert file_imported.kwargs["merge_summary"] == {"segments_copied": 1}
    assert file_imported.kwargs["principal_collision"] == {"source_entity_id": "a"}
    assert (
        file_imported.kwargs["merge_log_path"]
        == "/tmp/journal.merge/run/decisions.jsonl"
    )
    assert (
        file_imported.kwargs["merge_staging_path"] == "/tmp/journal.merge/run/staging"
    )
    assert file_imported.kwargs["summary_errors"] == [
        "segment 20260101/default/090000_300: segment copy failed"
    ]

    payload = json.loads(capsys.readouterr().out)
    assert payload["merge_summary"] == {"segments_copied": 1}
    assert payload["principal_collision"] == {"source_entity_id": "a"}
    assert payload["merge_log_path"] == "/tmp/journal.merge/run/decisions.jsonl"
    assert payload["merge_staging_path"] == "/tmp/journal.merge/run/staging"
    assert payload["summary_errors"] == [
        "segment 20260101/default/090000_300: segment copy failed"
    ]

    imported_path = tmp_path / "imports" / "20260303_120000" / "imported.json"
    imported = json.loads(imported_path.read_text(encoding="utf-8"))
    assert imported["merge_log_path"] == "/tmp/journal.merge/run/decisions.jsonl"
    assert imported["merge_staging_path"] == "/tmp/journal.merge/run/staging"
    assert imported["summary_errors"] == [
        "segment 20260101/default/090000_300: segment copy failed"
    ]


def test_acquire_merge_lock_reclaims_stale_pid(tmp_path, monkeypatch):
    lock_path = tmp_path / ".merge.lock"
    lock_path.write_text(
        json.dumps(
            {
                "pid": 999999,
                "started_at_utc": "2026-04-26T00:00:00+00:00",
                "kind": "file-import",
                "import_id": "stale",
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        journal_archive.os, "kill", MagicMock(side_effect=ProcessLookupError)
    )

    with journal_archive.acquire_merge_lock(tmp_path, "file-import", "fresh"):
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
        assert payload["import_id"] == "fresh"
        assert payload["pid"] == journal_archive.os.getpid()

    assert not lock_path.exists()
