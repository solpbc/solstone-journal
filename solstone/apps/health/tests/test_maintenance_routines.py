# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from unittest.mock import ANY, Mock

from solstone.apps.health import maintenance
from solstone.think.maintenance import (
    discover_routines,
    expected_schedule_entry,
)
from solstone.think.retention_executor import PruneResult


def _result(
    *,
    enabled: bool = True,
    files_deleted: int = 0,
    dirs_deleted: int = 0,
    bytes_freed: int = 0,
    errors: list[dict] | None = None,
    root_task_log: dict | None = None,
) -> PruneResult:
    return PruneResult(
        enabled=enabled,
        dry_run=False,
        days=30,
        cutoff_day="20260316",
        by_class={
            "tokens": {
                "files_deleted": files_deleted,
                "bytes_freed": bytes_freed,
                "dirs_deleted": dirs_deleted,
                "skipped": 0,
                "errors": [],
            }
        },
        by_day={},
        files_deleted=files_deleted,
        dirs_deleted=dirs_deleted,
        bytes_freed=bytes_freed,
        errors=errors or [],
        audit_written=False,
        partial_error=bool(errors),
        root_task_log=root_task_log
        or {
            "exists": False,
            "lines_total": 0,
            "lines_kept": 0,
            "lines_removed": 0,
            "unparseable_lines_kept": 0,
            "bytes_freed": 0,
            "rewritten": False,
            "errors": [],
        },
    )


def test_health_prune_logs_routine_is_discovered_with_expected_schedule_entry():
    routines = discover_routines()

    assert "health:prune-logs" in routines
    routine = routines["health:prune-logs"]
    assert routine.every == "daily"
    assert routine.max_runtime == "30m"
    assert expected_schedule_entry("health:prune-logs", routine) == {
        "cmd": ["journal", "maintenance", "run", "health:prune-logs"],
        "every": "daily",
        "enabled": True,
        "max_runtime": "30m",
    }


def test_prune_logs_routine_wrapper_disabled_and_real(monkeypatch, capsys):
    require_solstone = Mock()
    prune = Mock(side_effect=[_result(enabled=False), _result(files_deleted=2)])
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance.retention_executor, "prune_logs", prune)

    disabled_code = maintenance.run_prune_logs_routine([])
    real_code = maintenance.run_prune_logs_routine([])

    assert disabled_code == 0
    assert real_code == 0
    assert require_solstone.call_count == 2
    assert prune.call_count == 2
    prune.assert_called_with(ANY, days=None, dry_run=False)
    output = capsys.readouterr().out
    assert "prune-logs: disabled (no-op)" in output
    assert (
        "prune-logs: pruned 2 operational-log file(s), 0 cache dir(s), "
        "0 B cutoff=20260316"
    ) in output


def test_prune_logs_routine_rejects_nonpositive_days(monkeypatch, capsys):
    require_solstone = Mock()
    prune = Mock()
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance.retention_executor, "prune_logs", prune)

    code = maintenance.run_prune_logs_routine(["--days", "0"])

    assert code == 1
    require_solstone.assert_called_once_with()
    prune.assert_not_called()
    assert "--days must be a positive integer" in capsys.readouterr().err


def test_prune_logs_routine_prints_root_task_log_work(monkeypatch, capsys):
    require_solstone = Mock()
    prune = Mock(
        return_value=_result(
            bytes_freed=42,
            root_task_log={
                "exists": True,
                "lines_total": 4,
                "lines_kept": 1,
                "lines_removed": 3,
                "unparseable_lines_kept": 0,
                "bytes_freed": 42,
                "rewritten": True,
                "errors": [],
            },
        )
    )
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance.retention_executor, "prune_logs", prune)

    code = maintenance.run_prune_logs_routine([])

    assert code == 0
    output = capsys.readouterr().out
    assert (
        "prune-logs: pruned 0 operational-log file(s), 0 cache dir(s), 42 B" in output
    )
    assert "root_task_log: compacted 3 line(s), 42 B" in output


def test_prune_logs_routine_prints_partial_errors(monkeypatch, capsys):
    require_solstone = Mock()
    prune = Mock(
        return_value=_result(
            files_deleted=1,
            errors=[
                {
                    "reason": "delete_failed",
                    "message": "failed to delete path during pruning",
                    "hint": "check file ownership/permissions",
                }
            ],
        )
    )
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance.retention_executor, "prune_logs", prune)

    code = maintenance.run_prune_logs_routine([])

    assert code == 0
    output = capsys.readouterr().out
    assert "error: delete_failed: failed to delete path during pruning" in output
    assert "hint=check file ownership/permissions" in output
