# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import logging
from pathlib import Path
from unittest.mock import patch

import pytest


def test_load_sync_state_missing(tmp_path):
    """Returns None when no state file exists."""
    from solstone.think.importers.sync import load_sync_state

    assert load_sync_state(tmp_path, "plaud") is None


def test_save_and_load_sync_state(tmp_path):
    """Round-trip: save then load."""
    from solstone.think.importers.sync import load_sync_state, save_sync_state

    state = {
        "backend": "plaud",
        "last_sync": "2026-02-14T12:00:00",
        "files": {
            "abc123": {
                "filename": "test.opus",
                "status": "imported",
            }
        },
    }
    save_sync_state(tmp_path, "plaud", state)
    loaded = load_sync_state(tmp_path, "plaud")
    assert loaded == state


def test_save_sync_state_creates_imports_dir(tmp_path):
    """imports/ dir is created if missing."""
    from solstone.think.importers.sync import save_sync_state

    save_sync_state(tmp_path, "plaud", {"backend": "plaud"})
    assert (tmp_path / "imports" / "plaud.json").exists()


def test_save_sync_state_no_temp_files(tmp_path):
    """No .tmp files left after save."""
    from solstone.think.importers.sync import save_sync_state

    save_sync_state(tmp_path, "plaud", {"backend": "plaud"})
    imports = tmp_path / "imports"
    temps = list(imports.glob("*.tmp"))
    assert temps == []


def test_load_sync_state_corrupt_json(tmp_path):
    """Returns None on corrupt JSON."""
    from solstone.think.importers.sync import load_sync_state

    state_path = tmp_path / "imports" / "plaud.json"
    state_path.parent.mkdir(parents=True)
    state_path.write_text("not json{{{", encoding="utf-8")
    assert load_sync_state(tmp_path, "plaud") is None


def test_syncable_registry_exposes_only_supported_backends(caplog):
    """Discovers only Python-owned sync backends without loader warnings."""
    from solstone.think.importers.sync import SYNCABLE_REGISTRY, get_syncable_backends

    expected = {"plaud", "obsidian", "audio"}
    caplog.set_level(logging.WARNING, logger="solstone.think.importers.sync")
    assert set(SYNCABLE_REGISTRY) == expected
    backends = get_syncable_backends()
    names = {b.name for b in backends}
    assert names == expected
    assert len(backends) == 3
    assert not [
        record
        for record in caplog.records
        if record.name == "solstone.think.importers.sync"
        and record.levelno >= logging.WARNING
    ]


def test_plaud_protocol_conformance():
    """PlaudBackend satisfies SyncableBackend protocol."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import SyncableBackend

    assert isinstance(PlaudBackend(), SyncableBackend)


def test_plaud_sync_requires_token(tmp_path, monkeypatch):
    """Without token configured, PlaudBackend.sync() raises ValueError."""
    from solstone.think.importers.plaud import PlaudBackend

    monkeypatch.delenv("PLAUD_ACCESS_TOKEN", raising=False)
    with pytest.raises(ValueError, match="PLAUD_ACCESS_TOKEN"):
        PlaudBackend().sync(tmp_path)


def test_backends_cli_flag_lists_supported_backends_only(capsys, monkeypatch):
    """sol import --backends lists supported syncable backends."""
    import sys

    from solstone.think.importers.cli import main

    monkeypatch.setattr(sys, "argv", ["sol import", "--backends"])
    monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/test-journal")
    main()
    captured = capsys.readouterr()
    expected = {"plaud", "obsidian", "audio", "oura"}
    listed = {
        line.strip()
        for line in captured.out.splitlines()
        if line.strip() and not line.strip().endswith(":")
    }
    assert listed == expected
    assert "granola" not in captured.out


def test_sync_granola_reports_unknown_backend(monkeypatch, tmp_path):
    """Granola sync uses the generic unknown-backend path."""
    from solstone.think.importers.cli import _run_sync

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    with pytest.raises(SystemExit) as exc_info:
        _run_sync("granola", dry_run=True)

    assert (
        str(exc_info.value) == "Unknown sync backend: granola\n"
        "Available backends: plaud, obsidian, audio, oura"
    )


# ---------------------------------------------------------------------------
# timestamp_from_start_time
# ---------------------------------------------------------------------------


def test_timestamp_from_start_time_seconds():
    """Handles epoch seconds."""
    # 2026-01-15 10:30:00 local
    import datetime as dt

    from solstone.think.importers.plaud import timestamp_from_start_time

    epoch = dt.datetime(2026, 1, 15, 10, 30, 0).timestamp()
    ts = timestamp_from_start_time(epoch)
    assert ts == "20260115_103000"


def test_timestamp_from_start_time_millis():
    """Handles epoch milliseconds (Plaud format)."""
    import datetime as dt

    from solstone.think.importers.plaud import timestamp_from_start_time

    epoch_ms = dt.datetime(2026, 1, 15, 10, 30, 0).timestamp() * 1000
    ts = timestamp_from_start_time(epoch_ms)
    assert ts == "20260115_103000"


# ---------------------------------------------------------------------------
# match_existing_imports
# ---------------------------------------------------------------------------


def _create_import(journal_root: Path, timestamp: str, original_filename: str) -> None:
    """Helper: create a minimal imports/{timestamp}/import.json."""
    import_dir = journal_root / "imports" / timestamp
    import_dir.mkdir(parents=True, exist_ok=True)
    meta = {"original_filename": original_filename}
    (import_dir / "import.json").write_text(json.dumps(meta), encoding="utf-8")


def test_match_exact_filename(tmp_path):
    """Matches by exact filename."""
    from solstone.think.importers.plaud import match_existing_imports

    _create_import(tmp_path, "20260115_103000", "Team Meeting.opus")
    plaud_files = [
        {"id": "abc", "filename": "Team Meeting", "fullname": "hash1.opus"},
    ]
    matches = match_existing_imports(tmp_path, plaud_files)
    assert matches == {"abc": "20260115_103000"}


def test_match_sanitized_filename(tmp_path):
    """Matches sanitized filename with extension."""
    from solstone.think.importers.plaud import match_existing_imports

    _create_import(tmp_path, "20260115_103000", "Team_Meeting.opus")
    plaud_files = [
        {"id": "abc", "filename": "Team Meeting", "fullname": "hash1.opus"},
    ]
    matches = match_existing_imports(tmp_path, plaud_files)
    assert matches == {"abc": "20260115_103000"}


def test_match_no_match(tmp_path):
    """Returns empty dict when no match."""
    from solstone.think.importers.plaud import match_existing_imports

    _create_import(tmp_path, "20260115_103000", "Something Else.m4a")
    plaud_files = [
        {"id": "abc", "filename": "Team Meeting", "fullname": "hash1.opus"},
    ]
    matches = match_existing_imports(tmp_path, plaud_files)
    assert matches == {}


def test_match_no_imports_dir(tmp_path):
    """Returns empty dict when imports/ doesn't exist."""
    from solstone.think.importers.plaud import match_existing_imports

    plaud_files = [
        {"id": "abc", "filename": "Team Meeting", "fullname": "hash1.opus"},
    ]
    matches = match_existing_imports(tmp_path, plaud_files)
    assert matches == {}


def test_match_by_stem(tmp_path):
    """Matches by filename stem (without extension)."""
    from solstone.think.importers.plaud import match_existing_imports

    _create_import(tmp_path, "20260115_103000", "Team Meeting.m4a")
    plaud_files = [
        {"id": "abc", "filename": "Team Meeting", "fullname": "hash1.opus"},
    ]
    matches = match_existing_imports(tmp_path, plaud_files)
    assert matches == {"abc": "20260115_103000"}


# ---------------------------------------------------------------------------
# PlaudBackend.sync() — catalog mode
# ---------------------------------------------------------------------------


def _mock_list_files(_session, _token):
    """Return a small fake Plaud file list."""
    return [
        {
            "id": "file1",
            "filename": "Standup",
            "fullname": "aaa.opus",
            "filesize": 5000,
            "start_time": 1737000000000,
        },
        {
            "id": "file2",
            "filename": "Retro",
            "fullname": "bbb.opus",
            "filesize": 8000,
            "start_time": 1737100000000,
        },
    ]


def test_plaud_sync_dry_run(tmp_path, monkeypatch):
    """Dry-run sync fetches catalog and saves state."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    with patch(
        "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=True)

    assert result["total"] == 2
    assert result["available"] == 2
    assert result["skipped"] == 0
    assert result["imported"] == 0
    assert result["downloaded"] == 0

    # State was saved
    state = load_sync_state(tmp_path, "plaud")
    assert state is not None
    assert len(state["files"]) == 2
    assert state["files"]["file1"]["status"] == "available"


def test_plaud_sync_matches_existing(tmp_path, monkeypatch):
    """Sync matches existing imports and marks them imported."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")
    _create_import(tmp_path, "20260116_051320", "Standup.opus")

    with patch(
        "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=True)

    assert result["imported"] == 1
    assert result["available"] == 1

    state = load_sync_state(tmp_path, "plaud")
    assert state["files"]["file1"]["status"] == "imported"
    assert state["files"]["file1"]["import_timestamp"] == "20260116_051320"
    assert state["files"]["file2"]["status"] == "available"


def test_plaud_sync_incremental(tmp_path, monkeypatch):
    """Second sync preserves existing state and detects new files."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state, save_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    # Pre-seed state with file1 already imported
    save_sync_state(
        tmp_path,
        "plaud",
        {
            "backend": "plaud",
            "files": {
                "file1": {
                    "filename": "Standup",
                    "fullname": "aaa.opus",
                    "filesize": 5000,
                    "start_time": 1737000000000,
                    "status": "imported",
                    "import_timestamp": "20260116_051320",
                }
            },
        },
    )

    with patch(
        "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=True)

    assert result["total"] == 2
    assert result["imported"] == 1
    assert result["available"] == 1

    state = load_sync_state(tmp_path, "plaud")
    # file1 preserved as imported
    assert state["files"]["file1"]["status"] == "imported"
    # file2 detected as new available
    assert state["files"]["file2"]["status"] == "available"


def test_plaud_sync_promotes_manually_imported(tmp_path, monkeypatch):
    """Available file gets promoted to imported if manually imported between syncs."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state, save_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    # First sync: file2 is available
    save_sync_state(
        tmp_path,
        "plaud",
        {
            "backend": "plaud",
            "files": {
                "file1": {
                    "filename": "Standup",
                    "status": "imported",
                    "import_timestamp": "20260116_051320",
                },
                "file2": {
                    "filename": "Retro",
                    "fullname": "bbb.opus",
                    "filesize": 8000,
                    "start_time": 1737100000000,
                    "status": "available",
                },
            },
        },
    )

    # Simulate manual import of file2 (creates imports/*/import.json)
    _create_import(tmp_path, "20260117_134640", "Retro.opus")

    # Second sync: file2 should be promoted to imported
    with patch(
        "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=True)

    assert result["imported"] == 2
    assert result["available"] == 0

    state = load_sync_state(tmp_path, "plaud")
    assert state["files"]["file2"]["status"] == "imported"
    assert state["files"]["file2"]["import_timestamp"] == "20260117_134640"


def _mock_list_files_with_junk(_session, _token):
    """Return a file list including trashed and short recordings."""
    return [
        {
            "id": "good1",
            "filename": "Team Standup",
            "fullname": "aaa.opus",
            "filesize": 5000,
            "start_time": 1737000000000,
            "duration": 300000,
            "is_trash": False,
        },
        {
            "id": "trashed1",
            "filename": "Old Recording",
            "fullname": "bbb.opus",
            "filesize": 2000,
            "start_time": 1737100000000,
            "duration": 60000,
            "is_trash": True,
        },
        {
            "id": "short1",
            "filename": "Accidental Tap",
            "fullname": "ccc.opus",
            "filesize": 500,
            "start_time": 1737200000000,
            "duration": 5000,
            "is_trash": False,
        },
    ]


def test_plaud_sync_skips_trashed_and_short(tmp_path, monkeypatch):
    """Trashed and short recordings are auto-skipped."""
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    with patch(
        "solstone.think.importers.plaud.list_files",
        side_effect=_mock_list_files_with_junk,
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=True)

    assert result["total"] == 3
    assert result["available"] == 1
    assert result["skipped"] == 2
    assert result["imported"] == 0

    state = load_sync_state(tmp_path, "plaud")
    assert state["files"]["good1"]["status"] == "available"
    assert state["files"]["trashed1"]["status"] == "skipped"
    assert state["files"]["trashed1"]["skip_reason"] == "trashed"
    assert state["files"]["short1"]["status"] == "skipped"
    assert state["files"]["short1"]["skip_reason"] == "too_short"


def test_plaud_sync_save_calls_import_one_in_process(tmp_path, monkeypatch):
    from solstone.think.importers.plaud import PlaudBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    def fake_download(_session, _url, dest_path, progress_cb=None):
        dest_path.write_bytes(b"audio")
        if progress_cb:
            progress_cb()
        return True

    with (
        patch(
            "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
        ),
        patch(
            "solstone.think.importers.plaud.get_temp_url", return_value="https://temp"
        ),
        patch(
            "solstone.think.importers.plaud.download_to_file", side_effect=fake_download
        ),
        patch("solstone.think.importers.plaud.import_one") as import_mock,
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=False)

    assert result["downloaded"] == 2
    assert import_mock.call_count == 2
    for call in import_mock.call_args_list:
        assert call.kwargs["source"] == "plaud"
        assert call.kwargs["auto"] is True
        assert call.kwargs["timestamp"]
        assert call.kwargs["wait_for_processing"] is False

    state = load_sync_state(tmp_path, "plaud")
    assert state["files"]["file1"]["status"] == "imported"
    assert state["files"]["file2"]["status"] == "imported"


def test_plaud_sync_checkpoints_catalog_per_file(tmp_path, monkeypatch):
    from solstone.think.importers.plaud import PlaudBackend

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")
    saved_states = []

    def fake_download(_session, _url, dest_path, progress_cb=None):
        dest_path.write_bytes(b"audio")
        if progress_cb:
            progress_cb()
        return True

    def fake_save_sync_state(_journal_root, _backend, state):
        saved_states.append(json.loads(json.dumps(state)))

    with (
        patch(
            "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
        ),
        patch(
            "solstone.think.importers.plaud.get_temp_url", return_value="https://temp"
        ),
        patch(
            "solstone.think.importers.plaud.download_to_file", side_effect=fake_download
        ),
        patch(
            "solstone.think.importers.plaud.import_one",
            return_value={"segments": ["seg"]},
        ),
        patch(
            "solstone.think.importers.sync.save_sync_state",
            side_effect=fake_save_sync_state,
        ) as save_mock,
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=False)

    assert result["downloaded"] == 2
    assert save_mock.call_count == 3
    per_iteration_states = saved_states[:-1]
    assert any(
        any(info.get("status") == "imported" for info in state["files"].values())
        for state in per_iteration_states
    )


def test_plaud_sync_save_records_import_one_errors(tmp_path, monkeypatch):
    from solstone.think.importers.plaud import PlaudBackend

    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    def fake_download(_session, _url, dest_path, progress_cb=None):
        dest_path.write_bytes(b"audio")
        return True

    with (
        patch(
            "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
        ),
        patch(
            "solstone.think.importers.plaud.get_temp_url", return_value="https://temp"
        ),
        patch(
            "solstone.think.importers.plaud.download_to_file", side_effect=fake_download
        ),
        patch(
            "solstone.think.importers.plaud.import_one",
            side_effect=RuntimeError("import boom"),
        ),
    ):
        result = PlaudBackend().sync(tmp_path, dry_run=False)

    assert result["downloaded"] == 0
    assert len(result["errors"]) == 2
    assert all("import boom" in error for error in result["errors"])


@pytest.mark.parametrize(
    ("flags", "expected_dry_run"),
    [
        ([], True),
        (["--save"], False),
        (["--dry-run"], True),
        (["--save", "--dry-run"], True),
    ],
)
def test_sync_cli_dry_run_flag_matrix(flags, expected_dry_run, monkeypatch, tmp_path):
    """--dry-run is authoritative for sync dispatch dry-run behavior."""
    import sys

    from solstone.think.importers.cli import main

    monkeypatch.setattr(sys, "argv", ["sol import", "--sync", "plaud", *flags])
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with patch("solstone.think.importers.cli._run_sync") as mock_run:
        main()

    assert mock_run.call_args.kwargs["dry_run"] is expected_dry_run


def test_plaud_sync_cli_flag(capsys, monkeypatch, tmp_path):
    """sol import --sync plaud runs sync in dry-run mode."""
    import sys

    from solstone.think.importers.cli import main

    monkeypatch.setattr(sys, "argv", ["sol import", "--sync", "plaud"])
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("PLAUD_ACCESS_TOKEN", "test-token")

    with patch(
        "solstone.think.importers.plaud.list_files", side_effect=_mock_list_files
    ):
        main()

    captured = capsys.readouterr()
    assert "Total:" in captured.out
    assert "Available to import:" in captured.out
