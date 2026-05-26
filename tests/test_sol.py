# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for sol.py unified CLI."""

import os
import subprocess
import sys
import tomllib
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.think import sol_cli as sol
from solstone.think.sol_cli import JOURNAL_ACCESS_CMD_ERROR

REPO_ROOT = Path(__file__).resolve().parent.parent


def service_command_names() -> list[str]:
    return sorted(
        name for name, command in sol.COMMANDS.items() if command.surface == "service"
    )


def access_command_names() -> list[str]:
    return sorted(
        name for name, command in sol.COMMANDS.items() if command.surface == "access"
    )


def service_alias_names() -> list[str]:
    return [
        name
        for name, command_alias in sol.ALIASES.items()
        if command_alias.surface == "service"
    ]


def run_dispatch(monkeypatch, binary: str, name: str) -> dict[str, object]:
    result: dict[str, object] = {}

    def fake_run_command(module_path: str) -> int:
        result["module"] = module_path
        result["argv"] = sys.argv[:]
        return 0

    monkeypatch.setattr(sol, "run_command", fake_run_command)
    monkeypatch.setattr(sol.setproctitle, "setproctitle", lambda _title: None)
    monkeypatch.setattr(sys, "argv", [binary, name])

    with pytest.raises(SystemExit) as exc_info:
        if binary == "journal":
            sol.journal_main()
        else:
            sol.main()

    assert exc_info.value.code == 0
    return result


class TestResolveCommand:
    """Tests for resolve_command() function."""

    def test_resolve_known_command(self):
        """Test resolving a known command from registry."""
        module_path, preset_args, surface = sol.resolve_command("import")
        assert module_path == "solstone.think.importers.cli"
        assert preset_args == []
        assert surface == "access"

    def test_resolve_direct_module_path(self):
        """Test resolving a direct module path with dot."""
        module_path, preset_args, surface = sol.resolve_command(
            "solstone.think.importers.cli"
        )
        assert module_path == "solstone.think.importers.cli"
        assert preset_args == []
        assert surface == "service"

    def test_resolve_nested_module_path(self):
        """Test resolving a deeply nested module path."""
        module_path, preset_args, surface = sol.resolve_command(
            "solstone.observe.linux.observer"
        )
        assert module_path == "solstone.observe.linux.observer"
        assert preset_args == []
        assert surface == "service"

    def test_resolve_unknown_command_raises(self):
        """Test that unknown command raises ValueError."""
        with pytest.raises(ValueError) as exc_info:
            sol.resolve_command("nonexistent")
        assert "Unknown command: nonexistent" in str(exc_info.value)

    def test_resolve_alias_with_preset_args(self):
        """Test resolving an alias that includes preset arguments."""
        # Add a test alias
        sol.ALIASES["test-alias"] = sol.Alias(
            "solstone.think.indexer", ["--rescan"], "service"
        )
        try:
            module_path, preset_args, surface = sol.resolve_command("test-alias")
            assert module_path == "solstone.think.indexer"
            assert preset_args == ["--rescan"]
            assert surface == "service"
        finally:
            del sol.ALIASES["test-alias"]

    def test_alias_takes_precedence_over_command(self):
        """Test that aliases override commands with same name."""
        # Add an alias that shadows a command
        sol.ALIASES["import"] = sol.Alias(
            "solstone.think.cluster", ["--force"], "service"
        )
        try:
            module_path, preset_args, surface = sol.resolve_command("import")
            assert module_path == "solstone.think.cluster"
            assert preset_args == ["--force"]
            assert surface == "service"
        finally:
            del sol.ALIASES["import"]


class TestRunCommand:
    """Tests for run_command() function."""

    def test_run_command_success(self):
        """Test running a command that exits cleanly."""
        mock_module = MagicMock()
        mock_module.main = MagicMock(return_value=None)

        with patch("importlib.import_module", return_value=mock_module):
            exit_code = sol.run_command("test.module")
            assert exit_code == 0
            mock_module.main.assert_called_once()

    def test_run_command_with_system_exit(self):
        """Test running a command that calls sys.exit(0)."""
        mock_module = MagicMock()
        mock_module.main = MagicMock(side_effect=SystemExit(0))

        with patch("importlib.import_module", return_value=mock_module):
            exit_code = sol.run_command("test.module")
            assert exit_code == 0

    def test_run_command_with_nonzero_exit(self):
        """Test running a command that calls sys.exit(1)."""
        mock_module = MagicMock()
        mock_module.main = MagicMock(side_effect=SystemExit(1))

        with patch("importlib.import_module", return_value=mock_module):
            exit_code = sol.run_command("test.module")
            assert exit_code == 1

    def test_run_command_with_string_exit(self, capsys):
        """Test running a command that raises SystemExit with a string message."""
        mock_module = MagicMock()
        mock_module.main = MagicMock(side_effect=SystemExit("Error: something failed"))

        with patch("importlib.import_module", return_value=mock_module):
            exit_code = sol.run_command("test.module")
            assert exit_code == 1

        captured = capsys.readouterr()
        assert "Error: something failed" in captured.err

    def test_run_command_import_error(self):
        """Test handling ImportError for nonexistent module."""
        with patch(
            "importlib.import_module", side_effect=ImportError("No module named 'fake'")
        ):
            exit_code = sol.run_command("fake.module")
            assert exit_code == 1

    def test_run_command_no_main_function(self):
        """Test handling module without main() function."""
        mock_module = MagicMock(spec=[])  # No 'main' attribute

        with patch("importlib.import_module", return_value=mock_module):
            exit_code = sol.run_command("test.module")
            assert exit_code == 1

    def test_main_propagates_integer_return_code_via_real_subprocess(self, tmp_path):
        """Would fail on the parent commit because cmd_journal() returned 1 but sol exited 0."""
        env = {**os.environ, "SOLSTONE_JOURNAL": str(tmp_path)}
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "solstone.think.sol_cli",
                "config",
                "journal",
                "/tmp/with$dollar",
            ],
            capture_output=True,
            text=True,
            env=env,
            cwd=str(tmp_path),
        )

        assert result.returncode == 1


class TestGetStatus:
    """Tests for get_status() function."""

    def test_status_with_override(self, monkeypatch, tmp_path):
        """Test status when journal env is set and exists."""
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        status = sol.get_status()
        assert status["journal_path"] == str(tmp_path)
        assert status["journal_source"] == "env"
        assert status["journal_exists"] is True

    def test_status_with_nonexistent_journal(self, monkeypatch, tmp_path):
        """Test status when the journal env points to a nonexistent dir."""
        nonexistent = tmp_path / "nonexistent"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(nonexistent))

        status = sol.get_status()
        assert status["journal_path"] == str(nonexistent)
        assert status["journal_source"] == "env"
        assert status["journal_exists"] is False

    def test_status_without_override(self, monkeypatch):
        """Test status when no journal env is set uses source-tree fallback."""
        monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
        status = sol.get_status()
        assert status["journal_path"].endswith("/journal")
        assert status["journal_source"] == "source"
        assert isinstance(status["journal_exists"], bool)


class TestMain:
    """Tests for main() function."""

    def test_main_no_args_shows_help(self, monkeypatch, capsys):
        """Test that running with no args shows help."""
        monkeypatch.setattr(sys, "argv", ["sol"])
        monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/test")

        sol.main()

        captured = capsys.readouterr()
        assert "sol - solstone unified CLI" in captured.out
        assert "Usage: sol <command>" in captured.out

    def test_main_help_flag(self, monkeypatch, capsys):
        """Test --help flag shows help."""
        monkeypatch.setattr(sys, "argv", ["sol", "--help"])
        monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/test")

        sol.main()

        captured = capsys.readouterr()
        assert "sol - solstone unified CLI" in captured.out

    def test_main_help_command_without_question(self, monkeypatch, capsys):
        """Test bare 'help' command shows static help."""
        monkeypatch.setattr(sys, "argv", ["sol", "help"])
        monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/test")

        sol.main()

        captured = capsys.readouterr()
        assert "sol - solstone unified CLI" in captured.out

    def test_main_version_flag(self, monkeypatch, capsys):
        """Test --version flag shows version."""
        monkeypatch.setattr(sys, "argv", ["sol", "--version"])

        sol.main()

        captured = capsys.readouterr()
        assert "sol (solstone)" in captured.out

    def test_main_path_flag(self, monkeypatch, capsys):
        """Test --path flag prints resolved journal path."""
        monkeypatch.setattr(sys, "argv", ["sol", "--path"])
        monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/test-journal")

        sol.main()

        captured = capsys.readouterr()
        assert captured.out.strip() == "/tmp/test-journal"

    def test_main_path_flag_default(self, monkeypatch, capsys):
        """Test --path prints project root journal when no override set."""
        monkeypatch.setattr(sys, "argv", ["sol", "--path"])
        monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
        sol.main()

        captured = capsys.readouterr()
        path = captured.out.strip()
        assert path != ""
        assert path.endswith("/journal")

    def test_main_root_command(self, monkeypatch, capsys):
        """Test 'root' command prints the project root directory."""
        monkeypatch.setattr(sys, "argv", ["sol", "root"])

        sol.main()

        captured = capsys.readouterr()
        path = captured.out.strip()
        assert path != ""
        # root should NOT end with /journal — that's --path
        assert not path.endswith("/journal")
        # should be a parent of the journal path
        assert (
            path.endswith("/solstone")
            or "/solstone" in path
            or path.endswith("/worktree")
        )

    def test_main_unknown_command_exits(self, monkeypatch):
        """Test that unknown command exits with code 1."""
        monkeypatch.setattr(sys, "argv", ["sol", "unknown-command"])

        with pytest.raises(SystemExit) as exc_info:
            sol.main()
        assert exc_info.value.code == 1

    def test_main_adjusts_sys_argv(self, monkeypatch):
        """Test that sys.argv is adjusted for subcommand."""
        monkeypatch.setattr(sys, "argv", ["sol", "import", "--day", "20250101"])

        captured_argv = []

        def mock_main():
            captured_argv.extend(sys.argv)

        mock_module = MagicMock()
        mock_module.main = mock_main

        with patch("importlib.import_module", return_value=mock_module):
            with pytest.raises(SystemExit):
                sol.main()

        assert captured_argv[0] == "sol import"
        assert "--day" in captured_argv
        assert "20250101" in captured_argv


class TestCommandRegistry:
    """Tests for command registry completeness."""

    def test_all_commands_have_modules(self):
        """Test that all registered commands point to valid module paths."""
        for cmd, command in sol.COMMANDS.items():
            assert "." in command.module, f"Command '{cmd}' has invalid module path"

    def test_groups_contain_valid_commands(self):
        """Test that all commands in groups exist in registry."""
        for group in sol.help_groups():
            for cmd in group.commands:
                assert cmd in sol.COMMANDS, (
                    f"Command '{cmd}' in group '{group.heading}' not in registry"
                )

    def test_critical_commands_registered(self):
        """Test that critical commands are registered."""
        critical = ["import", "providers", "think", "indexer", "transcribe"]
        for cmd in critical:
            assert cmd in sol.COMMANDS, f"Critical command '{cmd}' not registered"

    def test_pyproject_declares_sol_and_journal_scripts(self):
        """Project scripts expose both top-level CLI entry points."""
        pyproject = tomllib.loads(
            (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
        )
        scripts = pyproject["project"]["scripts"]

        assert scripts["sol"] == "solstone.think.sol_cli:main"
        assert scripts["journal"] == "solstone.think.sol_cli:journal_main"
        assert scripts["sol"].startswith("solstone.think.sol_cli:")
        assert scripts["journal"].startswith("solstone.think.sol_cli:")

    def test_every_registry_entry_has_surface_tag(self):
        """All commands and aliases declare the CLI surface they belong to."""
        valid_surfaces = {"access", "service"}
        for name, command in sol.COMMANDS.items():
            assert command.surface in valid_surfaces, (
                f"Command '{name}' has invalid surface '{command.surface}'"
            )
        for name, command_alias in sol.ALIASES.items():
            assert command_alias.surface in valid_surfaces, (
                f"Alias '{name}' has invalid surface '{command_alias.surface}'"
            )

    def test_service_entries_dispatch_through_sol_and_journal(self, monkeypatch):
        """Service commands and aliases preserve module and preset args on both binaries."""
        for name in service_command_names():
            sol_result = run_dispatch(monkeypatch, "sol", name)
            journal_result = run_dispatch(monkeypatch, "journal", name)
            command = sol.COMMANDS[name]

            assert sol_result["module"] == command.module
            assert journal_result["module"] == command.module
            assert sol_result["argv"] == [f"sol {name}"]
            assert journal_result["argv"] == [f"journal {name}"]

        for name in service_alias_names():
            sol_result = run_dispatch(monkeypatch, "sol", name)
            journal_result = run_dispatch(monkeypatch, "journal", name)
            command_alias = sol.ALIASES[name]

            assert sol_result["module"] == command_alias.module
            assert journal_result["module"] == command_alias.module
            assert sol_result["argv"] == [f"sol {name}"] + command_alias.preset_args
            assert (
                journal_result["argv"]
                == [f"journal {name}"] + command_alias.preset_args
            )

    @pytest.mark.parametrize("name", access_command_names())
    def test_journal_rejects_access_tagged_commands(self, monkeypatch, capsys, name):
        """The journal binary exposes only service-tagged registry entries."""
        monkeypatch.setattr(
            sol,
            "run_command",
            lambda _module_path: pytest.fail("access command should not run"),
        )
        monkeypatch.setattr(sys, "argv", ["journal", name])

        with pytest.raises(SystemExit) as exc_info:
            sol.journal_main()

        captured = capsys.readouterr()
        assert exc_info.value.code == 2
        assert JOURNAL_ACCESS_CMD_ERROR.format(cmd=name) in captured.err

    def test_journal_help_lists_only_service_surface(self):
        """journal --help renders a flat service-only command list."""
        code = (
            "from solstone.think.sol_cli import journal_main; "
            "import sys; "
            "sys.argv = ['journal', '--help']; "
            "journal_main()"
        )
        result = subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            cwd=REPO_ROOT,
            timeout=60,
        )

        assert result.returncode == 0, result.stderr
        for name in service_command_names():
            assert name in result.stdout
        for name in access_command_names():
            assert name not in result.stdout
        assert "sol call" not in result.stdout

    def test_sol_help_still_lists_registry_groups_and_aliases(self):
        """sol --help still renders all registered top-level entries."""
        code = (
            "from solstone.think.sol_cli import main; "
            "import sys; "
            "sys.argv = ['sol', '--help']; "
            "main()"
        )
        result = subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            cwd=REPO_ROOT,
            timeout=60,
        )

        assert result.returncode == 0, result.stderr
        lines = result.stdout.splitlines()
        rendered_commands = {
            line.strip().split()[0]
            for line in lines
            if line.strip().split() and line.strip().split()[0] in sol.COMMANDS
        }
        expected_group_headers = {group.heading for group in sol.help_groups()}
        rendered_group_headers = {
            line for line in lines if line in expected_group_headers
        }

        rendered_aliases = set()
        in_aliases = False
        for line in lines:
            if line == sol.SOL_HELP_GROUP_ALIASES:
                in_aliases = True
                continue
            if in_aliases and not line.strip():
                break
            if in_aliases:
                name = line.strip().split()[0]
                if name in sol.ALIASES:
                    rendered_aliases.add(name)

        assert rendered_commands == set(sol.COMMANDS.keys())
        assert rendered_group_headers == expected_group_headers
        assert rendered_aliases == set(sol.ALIASES.keys())

    def test_setproctitle_prefix_uses_active_binary(self, monkeypatch):
        """The process title identifies whether sol or journal dispatched the command."""
        titles = []
        monkeypatch.setattr(sol, "run_command", lambda _module_path: 0)
        monkeypatch.setattr(sol.setproctitle, "setproctitle", titles.append)

        monkeypatch.setattr(sys, "argv", ["sol", "supervisor"])
        with pytest.raises(SystemExit):
            sol.main()

        monkeypatch.setattr(sys, "argv", ["journal", "supervisor"])
        with pytest.raises(SystemExit):
            sol.journal_main()

        assert titles[0].startswith("sol:")
        assert titles[1].startswith("journal:")
