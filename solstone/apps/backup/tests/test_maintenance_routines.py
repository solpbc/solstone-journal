# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from unittest.mock import Mock

from solstone.apps.backup import maintenance
from solstone.think.backup import engine
from solstone.think.maintenance import (
    discover_routines,
    expected_schedule_entry,
)
from solstone.think.offload import OffloadResult


def test_backup_routines_are_discovered_with_expected_schedule_entries() -> None:
    routines = discover_routines()

    assert "backup:run" in routines
    assert "backup:prune" in routines
    assert "backup:verify" in routines
    assert "backup:offload" in routines
    assert routines["backup:run"].every == "hourly"
    assert routines["backup:run"].max_runtime == "49h"
    assert routines["backup:prune"].every == "daily"
    assert routines["backup:prune"].max_runtime == "3h"
    assert routines["backup:verify"].every == "weekly"
    assert routines["backup:verify"].max_runtime == "90m"
    assert routines["backup:offload"].every == "daily"
    assert routines["backup:offload"].max_runtime == "7h"
    assert expected_schedule_entry("backup:run", routines["backup:run"]) == {
        "cmd": ["journal", "maintenance", "run", "backup:run"],
        "every": "hourly",
        "enabled": True,
        "max_runtime": "49h",
    }
    assert expected_schedule_entry("backup:prune", routines["backup:prune"]) == {
        "cmd": ["journal", "maintenance", "run", "backup:prune"],
        "every": "daily",
        "enabled": True,
        "max_runtime": "3h",
    }
    assert expected_schedule_entry("backup:verify", routines["backup:verify"]) == {
        "cmd": ["journal", "maintenance", "run", "backup:verify"],
        "every": "weekly",
        "enabled": True,
        "max_runtime": "90m",
    }
    assert expected_schedule_entry("backup:offload", routines["backup:offload"]) == {
        "cmd": ["journal", "maintenance", "run", "backup:offload"],
        "every": "daily",
        "enabled": True,
        "max_runtime": "7h",
    }


def test_backup_routine_wrappers_require_solstone_parse_empty_args_and_return_zero(
    monkeypatch,
    capsys,
) -> None:
    require_solstone = Mock()
    run_backup = Mock(
        return_value=engine.BackupResult(
            status="ok",
            snapshot_id="snap-1",
            error_reason=None,
        )
    )
    run_prune = Mock(
        return_value=engine.PruneResult(
            status="error",
            error_reason="locked",
        )
    )
    run_verification = Mock(
        return_value=engine.VerificationResult(
            status="ok",
            reason=None,
            checked_subset="7/52",
        )
    )
    run_offload = Mock(
        return_value=OffloadResult(
            status="ok",
            reason=None,
            files_marked=2,
            bytes_marked=50,
            files_already_marked=0,
            bytes_already_marked=0,
            ran_out_of_markable_media=False,
        )
    )
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance, "run_backup", run_backup)
    monkeypatch.setattr(maintenance, "run_prune", run_prune)
    monkeypatch.setattr(maintenance, "run_verification", run_verification)
    monkeypatch.setattr(maintenance, "run_offload", run_offload)

    backup_code = maintenance.run_backup_routine([])
    prune_code = maintenance.run_prune_routine([])
    verification_code = maintenance.run_verification_routine([])
    offload_code = maintenance.run_offload_routine([])

    assert backup_code == 0
    assert prune_code == 0
    assert verification_code == 0
    assert offload_code == 0
    assert require_solstone.call_count == 4
    run_backup.assert_called_once_with()
    run_prune.assert_called_once_with()
    run_verification.assert_called_once_with()
    run_offload.assert_called_once_with(dry_run=False)
    output = capsys.readouterr().out
    assert "backup: ok snapshot_id=snap-1" in output
    assert "backup prune: error reason=locked" in output
    assert "backup verify: ok subset=7/52" in output
    assert (
        "backup offload: ok files_marked=2 bytes_marked=50 files_already_marked=0 "
        "bytes_already_marked=0 bytes_released=0 ran_out_of_markable_media=False"
    ) in output


def test_offload_routine_wrapper_parses_dry_run_and_prints_stalls(
    monkeypatch,
    capsys,
) -> None:
    require_solstone = Mock()
    run_offload = Mock(
        return_value=OffloadResult(
            status="stalled",
            reason="locked",
            files_marked=0,
            bytes_marked=0,
            files_already_marked=0,
            bytes_already_marked=0,
            ran_out_of_markable_media=False,
            dry_run=True,
        )
    )
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance, "run_offload", run_offload)

    code = maintenance.run_offload_routine(["--dry-run"])

    assert code == 0
    require_solstone.assert_called_once_with()
    run_offload.assert_called_once_with(dry_run=True)
    assert (
        "backup offload: stalled reason=locked dry_run=true" in capsys.readouterr().out
    )


def test_verification_routine_wrapper_prints_skipped_and_error(
    monkeypatch,
    capsys,
) -> None:
    require_solstone = Mock()
    run_verification = Mock(
        side_effect=[
            engine.VerificationResult(
                status="skipped",
                reason=None,
                checked_subset=None,
            ),
            engine.VerificationResult(
                status="error",
                reason="locked",
                checked_subset=None,
            ),
        ]
    )
    monkeypatch.setattr(maintenance, "require_solstone", require_solstone)
    monkeypatch.setattr(maintenance, "run_verification", run_verification)

    skipped_code = maintenance.run_verification_routine([])
    error_code = maintenance.run_verification_routine([])

    assert skipped_code == 0
    assert error_code == 0
    assert require_solstone.call_count == 2
    assert run_verification.call_count == 2
    output = capsys.readouterr().out
    assert "backup verify: skipped" in output
    assert "backup verify: error reason=locked" in output


def test_request_backup_now_sends_supervisor_request_without_ref(monkeypatch) -> None:
    callosum_send = Mock(return_value=True)
    monkeypatch.setattr(engine, "callosum_send", callosum_send)

    assert engine.request_backup_now() is True
    assert engine.request_verification_now() is True

    callosum_send.assert_any_call(
        "supervisor",
        "request",
        cmd=["journal", "maintenance", "run", "backup:run"],
    )
    callosum_send.assert_any_call(
        "supervisor",
        "request",
        cmd=["journal", "maintenance", "run", "backup:verify"],
    )
    for call_args in callosum_send.call_args_list:
        assert "ref" not in call_args.kwargs
