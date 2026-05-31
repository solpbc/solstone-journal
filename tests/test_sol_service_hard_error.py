# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from solstone.think import sol_cli
from solstone.think.service import Reconciled


def service_command_names() -> list[str]:
    return sorted(
        name
        for name, command in sol_cli.COMMANDS.items()
        if command.surface == "service"
    )


def service_alias_names() -> list[str]:
    return sorted(
        name for name, alias in sol_cli.ALIASES.items() if alias.surface == "service"
    )


def _patch_no_stale_unit(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "solstone.think.service.reconcile_installed_unit",
        lambda: Reconciled(False, None, None, None),
    )


@pytest.mark.parametrize("name", service_command_names())
def test_sol_service_commands_hard_error(monkeypatch, capsys, name):
    _patch_no_stale_unit(monkeypatch)
    monkeypatch.setattr(
        sol_cli,
        "run_command",
        lambda _module_path: pytest.fail("service command should not run"),
    )
    monkeypatch.setattr(sys, "argv", ["sol", name])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    captured = capsys.readouterr()
    assert exc_info.value.code == 2
    assert sol_cli.SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd=name) in captured.err


@pytest.mark.parametrize("name", service_alias_names())
def test_sol_service_aliases_hard_error(monkeypatch, capsys, name):
    _patch_no_stale_unit(monkeypatch)
    monkeypatch.setattr(
        sol_cli,
        "run_command",
        lambda _module_path: pytest.fail("service alias should not run"),
    )
    monkeypatch.setattr(sys, "argv", ["sol", name])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    captured = capsys.readouterr()
    assert exc_info.value.code == 2
    assert sol_cli.SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd=name) in captured.err


def test_sol_access_commands_still_dispatch(monkeypatch):
    result: dict[str, object] = {}

    def fake_run_command(module_path: str) -> int:
        result["module"] = module_path
        result["argv"] = sys.argv[:]
        return 0

    monkeypatch.setattr(sol_cli, "run_command", fake_run_command)
    monkeypatch.setattr(sol_cli.setproctitle, "setproctitle", lambda _title: None)
    monkeypatch.setattr(sys, "argv", ["sol", "chat"])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    assert exc_info.value.code == 0
    assert result == {
        "module": sol_cli.COMMANDS["chat"].module,
        "argv": ["sol chat"],
    }


def test_stale_sol_unit_execs_journal(monkeypatch):
    execv = MagicMock(side_effect=RuntimeError("execv called"))
    monkeypatch.setattr(
        "solstone.think.service.reconcile_installed_unit",
        lambda: Reconciled(True, "sol", "supervisor", Path("unit")),
    )
    monkeypatch.setattr(
        "solstone.think.service._managed_wrapper",
        lambda binary: f"/tmp/{binary}",
    )
    monkeypatch.setattr(sol_cli.os, "execv", execv)
    monkeypatch.setattr(sys, "argv", ["sol", "supervisor", "5015"])

    with pytest.raises(RuntimeError, match="execv called"):
        sol_cli.main()

    # Shim routes through `journal start` (the canonical refresh-doing entry),
    # not `journal supervisor`, so the version-marker / wrapper / skill refresh
    # fires on this boot rather than waiting for the next restart.
    execv.assert_called_once_with("/tmp/journal", ["/tmp/journal", "start", "5015"])


def test_mismatched_stale_unit_does_not_unlock_human_sol_service(monkeypatch, capsys):
    monkeypatch.setattr(
        "solstone.think.service.reconcile_installed_unit",
        lambda: Reconciled(True, "sol", "heartbeat", Path("unit")),
    )
    monkeypatch.setattr(sys, "argv", ["sol", "supervisor"])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    captured = capsys.readouterr()
    assert exc_info.value.code == 2
    assert (
        sol_cli.SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd="supervisor") in captured.err
    )


def test_sol_service_hard_error_exit_code_is_2(monkeypatch):
    _patch_no_stale_unit(monkeypatch)
    monkeypatch.setattr(sys, "argv", ["sol", "supervisor"])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    assert exc_info.value.code == 2
