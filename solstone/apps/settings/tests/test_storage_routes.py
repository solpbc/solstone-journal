# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import shutil
from datetime import date, datetime, timedelta
from pathlib import Path

import pytest

import solstone.apps.settings.routes as settings_routes
import solstone.think.utils as think_utils
from solstone.convey import create_app
from solstone.convey.reasons import INVALID_CONFIG_VALUE
from solstone.think.retention_executor import PruneResult


@pytest.fixture(autouse=True)
def _executor(monkeypatch: pytest.MonkeyPatch) -> None:
    override = os.environ.get("SOLSTONE_RETENTION_BIN")
    if override and os.access(override, os.X_OK):
        monkeypatch.setenv("SOLSTONE_RETENTION_BIN", override)
        return

    found = shutil.which("solstone-retention")
    if found:
        monkeypatch.setenv("SOLSTONE_RETENTION_BIN", found)
        return

    root = Path(__file__).resolve().parents[4]
    for profile in ("debug", "release"):
        candidate = root / "core" / "target" / profile / "solstone-retention"
        if candidate.is_file() and os.access(candidate, os.X_OK):
            monkeypatch.setenv("SOLSTONE_RETENTION_BIN", str(candidate))
            return

    pytest.skip(
        "solstone-retention is not built; prune-logs runs through it, so this test "
        "has nothing real to assert against (cargo build -p solstone-core-retention-cli)"
    )


def _client(journal_path: Path):
    think_utils._journal_path_cache = None
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    return app.test_client()


def _read_config(journal_path: Path) -> dict:
    return json.loads(
        (journal_path / "config" / "journal.json").read_text(encoding="utf-8")
    )


def _old_day(days: int = 31) -> str:
    return (date.today() - timedelta(days=days)).strftime("%Y%m%d")


def _root_task_log_line(day: str, message: str) -> bytes:
    dt = datetime.strptime(day, "%Y%m%d").replace(hour=12)
    return f"{int(dt.timestamp())}\t{message}\n".encode("utf-8")


def _write(path: Path, content: str = "x") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


def test_storage_get_includes_journal_log_retention_defaults(settings_env):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    client = _client(journal_path)

    response = client.get("/app/settings/api/storage")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["retention"]["journal_logs"] == {"enabled": True, "days": 30}


def test_storage_put_journal_logs_preserves_raw_media_and_reverse(settings_env):
    journal_path, _config = settings_env(
        {
            "setup": {"completed_at": 1700000000000},
            "retention": {
                "raw_media": "days",
                "raw_media_days": 21,
                "per_stream": {
                    "desktop": {"raw_media": "processed", "raw_media_days": None}
                },
                "journal_logs": {"enabled": True, "days": 44},
            },
        }
    )
    client = _client(journal_path)

    response = client.put(
        "/app/settings/api/storage",
        json={"journal_logs": {"enabled": False, "days": 14}},
    )

    assert response.status_code == 200
    retention = _read_config(journal_path)["retention"]
    assert retention["journal_logs"] == {"enabled": False, "days": 14}
    assert retention["raw_media"] == "days"
    assert retention["raw_media_days"] == 21
    assert retention["per_stream"] == {
        "desktop": {"raw_media": "processed", "raw_media_days": None}
    }

    raw_response = client.put(
        "/app/settings/api/storage",
        json={"raw_media": "processed", "raw_media_days": None},
    )

    assert raw_response.status_code == 200
    retention = _read_config(journal_path)["retention"]
    assert retention["raw_media"] == "processed"
    assert retention["raw_media_days"] is None
    assert retention["journal_logs"] == {"enabled": False, "days": 14}


@pytest.mark.parametrize(
    "payload",
    [
        {"journal_logs": "bad"},
        {"journal_logs": {"days": 0}},
        {"journal_logs": {"days": "14"}},
        {"journal_logs": {"enabled": "false"}},
    ],
)
def test_storage_put_journal_logs_validates_values(settings_env, payload):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    client = _client(journal_path)

    response = client.put("/app/settings/api/storage", json=payload)

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == INVALID_CONFIG_VALUE.code


def test_storage_prune_logs_dry_run_serializes_result_and_deletes_nothing(
    settings_env,
):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    old_token = _write(journal_path / "tokens" / f"{_old_day()}.jsonl")
    client = _client(journal_path)

    response = client.post("/app/settings/api/storage/prune-logs", json={})

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["enabled"] is True
    assert payload["dry_run"] is True
    assert payload["days"] == 30
    assert payload["cutoff_day"]
    assert payload["files_deleted"] == 1
    assert payload["dirs_deleted"] == 0
    assert payload["bytes_freed"] == 1
    assert payload["bytes_freed_human"] == "1 B"
    assert payload["by_class"]["tokens"]["files_deleted"] == 1
    assert payload["by_day"][_old_day()]["files_deleted"] == 1
    assert payload["errors"] == []
    assert payload["audit_written"] is False
    assert payload["partial_error"] is False
    assert old_token.exists()


def test_storage_prune_logs_serializes_root_task_log_dry_run(settings_env):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    old_line = _root_task_log_line(_old_day(), "old root line")
    root_log = journal_path / "task_log.txt"
    root_log.write_bytes(old_line)
    old_retention_line = (
        f'{{"timestamp":"{datetime.strptime(_old_day(), "%Y%m%d"):%Y-%m-%d}T12:00:00"}}\n'
    ).encode("utf-8")
    retention_log = journal_path / "health" / "retention.log"
    retention_log.parent.mkdir(parents=True, exist_ok=True)
    retention_log.write_bytes(old_retention_line)
    client = _client(journal_path)

    response = client.post("/app/settings/api/storage/prune-logs", json={})

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["files_deleted"] == 0
    assert payload["dirs_deleted"] == 0
    assert payload["bytes_freed"] == len(old_line) + len(old_retention_line)
    assert payload["root_task_log"]["exists"] is True
    assert payload["root_task_log"]["lines_removed"] == 1
    assert payload["root_task_log"]["bytes_freed"] == len(old_line)
    assert payload["root_task_log"]["rewritten"] is False
    assert payload["retention_log"]["exists"] is True
    assert payload["retention_log"]["lines_removed"] == 1
    assert payload["retention_log"]["bytes_freed"] == len(old_retention_line)
    assert payload["retention_log"]["rewritten"] is False
    assert root_log.read_bytes() == old_line
    assert retention_log.read_bytes() == old_retention_line


def test_storage_prune_logs_disabled_config_deletes_nothing(settings_env):
    journal_path, _config = settings_env(
        {
            "setup": {"completed_at": 1700000000000},
            "retention": {"journal_logs": {"enabled": False, "days": 30}},
        }
    )
    old_token = _write(journal_path / "tokens" / f"{_old_day()}.jsonl")
    client = _client(journal_path)

    response = client.post(
        "/app/settings/api/storage/prune-logs",
        json={"dry_run": False},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["enabled"] is False
    assert payload["dry_run"] is False
    assert payload["files_deleted"] == 0
    assert payload["audit_written"] is False
    assert old_token.exists()
    assert not (journal_path / "health" / "pruning-runs").exists()


def test_storage_prune_logs_validates_days(settings_env):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    client = _client(journal_path)

    response = client.post(
        "/app/settings/api/storage/prune-logs",
        json={"dry_run": True, "days": 0},
    )

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == INVALID_CONFIG_VALUE.code


def test_storage_prune_logs_serializes_partial_errors(settings_env, monkeypatch):
    journal_path, _config = settings_env({"setup": {"completed_at": 1700000000000}})
    client = _client(journal_path)
    result = PruneResult(
        enabled=True,
        dry_run=False,
        days=30,
        cutoff_day="20260526",
        by_class={
            "tokens": {
                "files_deleted": 1,
                "bytes_freed": 1,
                "dirs_deleted": 0,
                "skipped": 0,
                "errors": ["failed"],
            }
        },
        by_day={"20260525": {"files_deleted": 1, "bytes_freed": 1, "dirs_deleted": 0}},
        files_deleted=1,
        dirs_deleted=0,
        bytes_freed=1,
        errors=[
            {
                "class": "tokens",
                "path": "tokens/20260525.jsonl",
                "day": "20260525",
                "reason": "the log entry could not be removed: permission denied",
                "message": "the log entry could not be removed: permission denied",
                "hint": None,
            }
        ],
        audit_written=False,
        partial_error=True,
    )
    monkeypatch.setattr(settings_routes, "prune_logs", lambda *_args, **_kwargs: result)

    response = client.post(
        "/app/settings/api/storage/prune-logs",
        json={"dry_run": False, "days": 30},
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["partial_error"] is True
    assert payload["audit_written"] is False
    assert payload["errors"][0]["reason"] == "the log entry could not be removed: permission denied"
    assert payload["by_class"]["tokens"]["errors"] == ["failed"]
