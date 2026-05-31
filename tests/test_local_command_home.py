# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import MagicMock

import pytest
from typer.testing import CliRunner

from solstone.think import sol_cli
from solstone.think.service import Reconciled

LOCAL_COMMANDS = {
    "navigate": "solstone.think.tools.navigate",
    "routines": "solstone.think.tools.routines",
    "identity": "solstone.think.tools.sol",
    "install-provider": "solstone.think.install_provider",
}


@pytest.mark.parametrize(("command", "module"), LOCAL_COMMANDS.items())
def test_local_commands_resolve_as_service(command: str, module: str) -> None:
    resolved_module, preset_args, surface = sol_cli.resolve_command(command)

    assert resolved_module == module
    assert preset_args == []
    assert surface == "service"


@pytest.mark.parametrize(
    ("command", "extra_args"),
    [
        ("navigate", ["/x"]),
        ("routines", ["list"]),
        ("identity", ["self"]),
        ("install-provider", ["local"]),
    ],
)
def test_local_commands_run_under_journal(
    monkeypatch, command: str, extra_args: list[str]
) -> None:
    captured = {}

    def run_command(module_path: str) -> int:
        captured["module_path"] = module_path
        captured["argv"] = list(sys.argv)
        return 0

    monkeypatch.setattr(sol_cli, "run_command", run_command)
    monkeypatch.setattr(sys, "argv", ["journal", command, *extra_args])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.journal_main()

    assert exc_info.value.code == 0
    assert captured == {
        "module_path": LOCAL_COMMANDS[command],
        "argv": [f"journal {command}", *extra_args],
    }


@pytest.mark.parametrize("command", LOCAL_COMMANDS)
def test_local_commands_reject_under_sol(monkeypatch, capsys, command: str) -> None:
    monkeypatch.setattr(
        "solstone.think.service.reconcile_installed_unit",
        lambda: Reconciled(False, None, None, None),
    )
    monkeypatch.setattr(sys, "argv", ["sol", command])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    assert exc_info.value.code == 2
    assert (
        sol_cli.SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd=command)
        in capsys.readouterr().err
    )


def test_local_command_redirect_does_not_exec_stale_unrelated_unit(
    monkeypatch, capsys
) -> None:
    execv = MagicMock()
    monkeypatch.setattr(
        "solstone.think.service.reconcile_installed_unit",
        lambda: Reconciled(True, "sol", "supervisor", Path("unit")),
    )
    monkeypatch.setattr(sol_cli.os, "execv", execv)
    monkeypatch.setattr(sys, "argv", ["sol", "navigate"])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.main()

    assert exc_info.value.code == 2
    assert (
        sol_cli.SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd="navigate")
        in capsys.readouterr().err
    )
    execv.assert_not_called()


def test_local_commands_are_journal_help_only(monkeypatch, capsys) -> None:
    monkeypatch.setattr(sol_cli, "print_status", lambda: None)

    sol_cli.print_journal_help()
    journal_help = capsys.readouterr().out
    for command in LOCAL_COMMANDS:
        assert command in journal_help
        assert sol_cli.COMMANDS[command].surface == "service"

    sol_cli.print_help()
    sol_help = capsys.readouterr().out
    owner_groups = sol_help.split("Apps (sol call <app>):", 1)[0]
    for command in LOCAL_COMMANDS:
        assert f"  {command:16}" not in owner_groups


@pytest.mark.parametrize(
    ("args", "pointer"),
    [
        (["navigate", "/x"], "journal navigate"),
        (["routines", "list"], "journal routines"),
        (["identity"], "journal identity"),
        (["settings", "providers", "install", "local"], "journal install-provider"),
    ],
)
def test_old_sol_call_paths_redirect(args: list[str], pointer: str) -> None:
    from solstone.think.call import call_app

    result = CliRunner().invoke(call_app, args)

    assert result.exit_code != 0
    combined = result.output + result.stderr
    assert "journal " in combined
    assert pointer in combined


def test_old_sol_call_help_lists_moved_stubs() -> None:
    from solstone.think.call import call_app

    result = CliRunner().invoke(call_app, ["--help"])

    assert result.exit_code == 0
    assert "Moved to `journal navigate`." in result.output
    assert "Moved to `journal routines`." in result.output
    assert "Moved to `journal identity`." in result.output
