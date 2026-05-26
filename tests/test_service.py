# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think/service.py - cross-platform service management."""

from __future__ import annotations

import json
import plistlib
import subprocess
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.think import service


class TestPlatform:
    def test_darwin(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "darwin")
        assert service._platform() == "darwin"

    def test_linux(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        assert service._platform() == "linux"

    def test_unsupported(self, monkeypatch, capsys):
        monkeypatch.setattr(sys, "platform", "win32")
        with pytest.raises(SystemExit):
            service._platform()
        assert "unsupported platform" in capsys.readouterr().err


class TestPlistGeneration:
    def test_round_trip(self, tmp_path):
        journal_path = str(tmp_path / "journal")
        service_log = str(Path(journal_path) / "health" / "service.log")
        env = {
            "HOME": "/Users/test",
            "PATH": "/usr/bin",
            "PYTHONUNBUFFERED": "1",
        }
        data = service._generate_plist(env, journal_path=journal_path)
        plist = plistlib.loads(data)
        assert plist["Label"] == "org.solpbc.solstone"
        assert plist["ProgramArguments"][0] == str(
            Path.home() / ".local" / "bin" / "journal"
        )
        assert plist["ProgramArguments"][1] == "supervisor"
        assert plist["EnvironmentVariables"] == env
        assert plist["EnvironmentVariables"]["PYTHONUNBUFFERED"] == "1"
        assert plist["KeepAlive"] == {"SuccessfulExit": False}
        assert plist["RunAtLoad"] is True
        assert plist["StandardOutPath"] == service_log
        assert plist["StandardErrorPath"] == service_log

    def test_keep_alive_is_sticky_stop(self, tmp_path):
        env = {
            "HOME": "/Users/test",
            "PATH": "/usr/bin",
        }
        data = service._generate_plist(env, journal_path=str(tmp_path / "journal"))
        plist = plistlib.loads(data)

        # Clean exits stay stopped; non-zero exits respawn.
        assert isinstance(plist["KeepAlive"], dict)
        assert plist["KeepAlive"]["SuccessfulExit"] is False

    def test_invalid_journal_path_rejected(self):
        with pytest.raises(ValueError, match="shell-active character"):
            service._generate_plist({}, journal_path="/tmp/bad\npath")


class TestSystemdUnit:
    def test_unit_content(self, tmp_path):
        journal_path = str(tmp_path / "journal")
        service_log = str(Path(journal_path) / "health" / "service.log")
        env = {
            "HOME": "/home/test",
            "PATH": "/usr/bin",
            "PYTHONUNBUFFERED": "1",
        }
        unit = service._generate_systemd_unit(env, journal_path=journal_path)
        lines = unit.splitlines()

        # Section headers must start at column 0 (no leading whitespace)
        assert "[Unit]" == lines[0]
        assert any(line == "[Service]" for line in lines)
        assert any(line == "[Install]" for line in lines)

        assert "Type=notify" in unit
        assert "Restart=on-failure" in unit
        assert "StartLimitIntervalSec=120" in unit
        assert "StartLimitBurst=10" in unit
        assert "KillMode=control-group" in unit
        assert "TimeoutStopSec=30" in unit
        assert f"StandardOutput=append:{service_log}" in unit
        assert "StandardError=inherit" in unit
        assert (
            f"ExecStart={Path.home() / '.local' / 'bin' / 'journal'} supervisor 5015"
            in unit
        )
        assert "supervisor" in unit
        assert "Environment=HOME=/home/test" in unit
        assert "Environment=PATH=/usr/bin" in unit
        assert "Environment=PYTHONUNBUFFERED=1" in unit
        assert "SOLSTONE_JOURNAL" not in unit
        assert "WantedBy=default.target" in unit

    def test_native_stdio_redirection(self, tmp_path):
        journal_path = str(tmp_path / "journal")
        service_log = str(Path(journal_path) / "health" / "service.log")
        env = {
            "HOME": "/home/test",
            "PATH": "/usr/bin",
        }

        unit = service._generate_systemd_unit(env, journal_path=journal_path)

        assert f"StandardOutput=append:{service_log}" in unit
        assert "StandardError=inherit" in unit

    def test_invalid_journal_path_rejected(self):
        with pytest.raises(ValueError, match="shell-active character"):
            service._generate_systemd_unit({}, journal_path="/tmp/bad$path")


class TestLogs:
    def test_reads_service_log(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "darwin")
        health_dir = tmp_path / "health"
        health_dir.mkdir(parents=True)
        service_log = health_dir / "service.log"
        service_log.write_text("first line\nsecond line\n", encoding="utf-8")
        monkeypatch.setattr(service, "get_journal", lambda: str(tmp_path))

        result = service._logs(follow=False)

        assert result == 0
        captured = capsys.readouterr()
        assert captured.out == "=== service.log ===\nfirst line\nsecond line\n\n"
        assert captured.err == ""


class TestEnvCollection:
    def test_no_api_keys_in_env(self, monkeypatch, tmp_path):
        """Service env must NOT contain API keys — they load at runtime via setup_cli."""
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        config_dir = tmp_path / "config"
        config_dir.mkdir(exist_ok=True)
        (config_dir / "journal.json").write_text(
            json.dumps(
                {
                    "env": {
                        "ANTHROPIC_API_KEY": "sk-test",
                        "OPENAI_API_KEY": "sk-openai",
                        "GOOGLE_API_KEY": "gk-test",
                    }
                }
            )
        )

        env = service._collect_env()
        assert "ANTHROPIC_API_KEY" not in env
        assert "OPENAI_API_KEY" not in env
        assert "GOOGLE_API_KEY" not in env
        assert env["PYTHONUNBUFFERED"] == "1"

    def test_includes_venv_in_path(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setenv("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        monkeypatch.setattr(
            sys, "executable", str(tmp_path / ".venv" / "bin" / "python")
        )

        env = service._collect_env()
        venv_bin = str(Path(sys.executable).parent)
        assert env["PATH"] == (
            f"{venv_bin}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
        )

    def test_path_fallback_when_unset(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.delenv("PATH", raising=False)
        monkeypatch.setattr(
            sys, "executable", str(tmp_path / ".venv" / "bin" / "python")
        )

        env = service._collect_env()
        venv_bin = str(Path(sys.executable).parent)
        assert env["PATH"] == f"{venv_bin}:/usr/local/bin:/usr/bin:/bin"

    def test_path_deduplicates_venv_bin(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            sys, "executable", str(tmp_path / ".venv" / "bin" / "python")
        )
        venv_bin = str(Path(sys.executable).parent)
        monkeypatch.setenv("PATH", f"{venv_bin}:/usr/local/bin:/usr/bin:/bin")

        env = service._collect_env()
        parts = env["PATH"].split(":")
        assert parts[0] == venv_bin
        assert parts.count(venv_bin) == 1

    def test_journal_env_not_propagated(self, monkeypatch, tmp_path):
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        env = service._collect_env()
        assert "SOLSTONE_JOURNAL" not in env


class TestServiceHelpers:
    def test_service_is_installed_true_linux(self, monkeypatch, tmp_path):
        unit_path = tmp_path / "solstone.service"
        unit_path.write_text("", encoding="utf-8")
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        monkeypatch.setattr(service, "_unit_path", lambda: unit_path)
        assert service.service_is_installed() is True

    def test_service_is_installed_false_linux(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        monkeypatch.setattr(
            service, "_unit_path", lambda: tmp_path / "missing" / "solstone.service"
        )
        assert service.service_is_installed() is False

    def test_service_is_installed_true_darwin(self, monkeypatch, tmp_path):
        plist_path = tmp_path / "org.solpbc.solstone.plist"
        plist_path.write_text("", encoding="utf-8")
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service, "_plist_path", lambda: plist_path)
        assert service.service_is_installed() is True

    def test_service_is_installed_false_darwin(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "missing" / "org.solpbc.solstone.plist",
        )
        assert service.service_is_installed() is False

    def test_service_is_running_false_fast_when_not_installed(self, monkeypatch):
        run_mock = MagicMock()
        monkeypatch.setattr(service, "service_is_installed", lambda: False)
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is False
        run_mock.assert_not_called()

    def test_service_is_running_true_linux(self, monkeypatch):
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        run_mock = MagicMock(return_value=MagicMock(stdout="active\n"))
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is True

    @pytest.mark.parametrize("state", ["inactive\n", "failed\n"])
    def test_service_is_running_false_linux(self, monkeypatch, state):
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        run_mock = MagicMock(return_value=MagicMock(stdout=state))
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is False

    def test_service_is_running_true_darwin(self, monkeypatch):
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        launchctl_stdout = """gui/501/org.solpbc.solstone = {
\tactive count = 1
\tpath = /Users/jer/Library/LaunchAgents/org.solpbc.solstone.plist
\ttype = LaunchAgent
\tstate = running
\tprogram = /Users/jer/.local/bin/sol
\tpid = 12345
\tdomain = gui/501
\tasid = 100012
\tlast exit code = 0
\trun interval = 0
\tactive transactions = 0
\tdefault environment = {
\t\tPATH => /usr/bin:/bin
\t}
\tenvironment = {
\t\tHOME => /Users/jer
\t}
\tdomain = gui/501
\tminimum runtime = 10
\texit timeout = 5
\tendpoints = {
\t}
\tevent triggers = {
\t}
\tpid local dispatch queue = {
\t\tjob state = running
\t}
}
"""
        run_mock = MagicMock(
            return_value=MagicMock(returncode=0, stdout=launchctl_stdout)
        )
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is True

    def test_service_is_running_false_when_not_loaded_darwin(self, monkeypatch):
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        run_mock = MagicMock(return_value=MagicMock(returncode=1, stdout=""))
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is False

    def test_service_is_running_false_when_loaded_but_stopped_darwin(self, monkeypatch):
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        launchctl_stdout = """gui/501/org.solpbc.solstone = {
\tactive count = 0
\tpath = /Users/jer/Library/LaunchAgents/org.solpbc.solstone.plist
\ttype = LaunchAgent
\tstate = not running
\tprogram = /Users/jer/.local/bin/sol
\tdomain = gui/501
\tasid = 100012
\trun interval = 0
\tactive transactions = 0
\tdefault environment = {
\t\tPATH => /usr/bin:/bin
\t}
\tenvironment = {
\t\tHOME => /Users/jer
\t}
\tdomain = gui/501
\tminimum runtime = 10
\texit timeout = 5
\tendpoints = {
\t}
\tevent triggers = {
\t}
\tpid local dispatch queue = {
\t\tjob state = exited
\t}
\tlast exit code = 0
}
"""
        run_mock = MagicMock(
            return_value=MagicMock(returncode=0, stdout=launchctl_stdout)
        )
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is False


class TestStatus:
    def test_not_installed_linux(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(
            service, "_unit_path", lambda: tmp_path / "nonexistent.service"
        )

        result = service._status()
        assert result == 1
        output = capsys.readouterr().out
        assert "not installed" in output

    def test_not_installed_darwin(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "darwin")
        monkeypatch.setattr(
            service, "_plist_path", lambda: tmp_path / "nonexistent.plist"
        )

        result = service._status()
        assert result == 1
        output = capsys.readouterr().out
        assert "not installed" in output


class TestRestart:
    def test_if_installed_noop_when_not_installed_linux(
        self, monkeypatch, tmp_path, capsys
    ):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(
            service, "_unit_path", lambda: tmp_path / "nonexistent.service"
        )

        result = service._restart(if_installed=True)
        assert result == 0
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_if_installed_noop_when_not_installed_darwin(
        self, monkeypatch, tmp_path, capsys
    ):
        monkeypatch.setattr(sys, "platform", "darwin")
        monkeypatch.setattr(
            service, "_plist_path", lambda: tmp_path / "nonexistent.plist"
        )

        result = service._restart(if_installed=True)
        assert result == 0
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_errors_when_not_installed_linux(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(
            service, "_unit_path", lambda: tmp_path / "nonexistent.service"
        )

        result = service._restart()
        assert result == 1
        assert "not installed" in capsys.readouterr().err

    def test_errors_when_not_installed_darwin(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "darwin")
        monkeypatch.setattr(
            service, "_plist_path", lambda: tmp_path / "nonexistent.plist"
        )

        result = service._restart()
        assert result == 1
        assert "not installed" in capsys.readouterr().err

    def test_linux_happy_path_narrates(self, capsys, monkeypatch):
        """_restart prints stopping-old + restarted narration on the Linux happy path."""
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "clear_ready", MagicMock())
        monkeypatch.setattr(service, "wait_ready", MagicMock(return_value={}))
        monkeypatch.setattr(
            "subprocess.run",
            lambda *a, **kw: subprocess.CompletedProcess(
                args=a, returncode=0, stdout="", stderr=""
            ),
        )
        result = service._restart()
        assert result == 0
        out = capsys.readouterr().out
        assert "Stopping old supervisor" in out
        assert "Service restarted." in out

    def test_restart_darwin_waits_for_readiness(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(
            "subprocess.run",
            lambda *a, **kw: subprocess.CompletedProcess(
                args=a, returncode=0, stdout="", stderr=""
            ),
        )
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value={"pid": 123})
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)

        assert service._restart() == 0
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)

    def test_restart_linux_waits_for_readiness(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(
            "subprocess.run",
            lambda *a, **kw: subprocess.CompletedProcess(
                args=a, returncode=0, stdout="", stderr=""
            ),
        )
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value={"pid": 123})
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)

        assert service._restart() == 0
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)


class TestUp:
    def test_up_waits_for_readiness_after_start(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "service_is_running", lambda: False)
        start = MagicMock(return_value=0)
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value={"pid": 123})
        status = MagicMock(return_value=0)
        monkeypatch.setattr(service, "_start", start)
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)
        monkeypatch.setattr(service, "_status", status)

        assert service._up(port=5015) == 0
        start.assert_called_once_with()
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_called_once_with()

    def test_up_already_running_waits_for_readiness(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "linux")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "service_is_running", lambda: True)
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value={"pid": 123})
        status = MagicMock(return_value=0)
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)
        monkeypatch.setattr(service, "_status", status)

        assert service._up(port=5015) == 0
        clear_ready.assert_not_called()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_called_once_with()


class TestInstall:
    def test_darwin_clears_readiness_before_bootstrap(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        calls = []

        def clear_ready():
            calls.append("clear_ready")

        def run(command, **kwargs):
            del kwargs
            if command[:2] == ["launchctl", "bootstrap"]:
                calls.append("bootstrap")
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert calls.index("clear_ready") < calls.index("bootstrap")

    def test_linux_idempotent(self, monkeypatch, tmp_path, capsys):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        unit_path = tmp_path / "solstone.service"
        monkeypatch.setattr(service, "_unit_path", lambda: unit_path)

        with patch("solstone.think.service.subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0)

            result = service._install()
            assert result == 0
            assert unit_path.exists()

            result = service._install()
            assert result == 0
            assert unit_path.exists()

        assert "Wrote" in capsys.readouterr().out


class TestLingerCheck:
    def test_warns_when_linger_disabled(self, capsys):
        mock_result = MagicMock(returncode=0, stdout="Linger=no\n")
        with patch("solstone.think.service.subprocess.run", return_value=mock_result):
            service._check_linger()
        output = capsys.readouterr().out
        assert "linger is not enabled" in output.lower()

    def test_silent_when_linger_enabled(self, capsys):
        mock_result = MagicMock(returncode=0, stdout="Linger=yes\n")
        with patch("solstone.think.service.subprocess.run", return_value=mock_result):
            service._check_linger()
        output = capsys.readouterr().out
        assert "linger" not in output.lower()

    def test_silent_when_loginctl_missing(self, capsys):
        with patch(
            "solstone.think.service.subprocess.run", side_effect=FileNotFoundError
        ):
            service._check_linger()
        output = capsys.readouterr().out
        assert output == ""


class TestRegistry:
    def test_service_command_registered(self):
        from solstone.think import sol_cli as sol

        assert "service" in sol.COMMANDS
        assert sol.COMMANDS["service"].module == "solstone.think.service"

    def test_up_alias(self):
        from solstone.think import sol_cli as sol

        assert "up" in sol.ALIASES
        alias = sol.ALIASES["up"]
        assert (alias.module, alias.preset_args) == ("solstone.think.service", ["up"])

    def test_down_alias(self):
        from solstone.think import sol_cli as sol

        assert "down" in sol.ALIASES
        alias = sol.ALIASES["down"]
        assert (alias.module, alias.preset_args) == ("solstone.think.service", ["down"])

    def test_service_group_exists(self):
        from solstone.think import sol_cli as sol

        assert sol.service_help_group().heading == sol.SOL_HELP_GROUP_SERVICE_HEADING
        assert "service" in sol.service_help_group().commands


class TestMain:
    def test_no_args_shows_usage(self, monkeypatch, capsys):
        monkeypatch.setattr(sys, "argv", ["journal service"])
        with pytest.raises(SystemExit):
            service.main()
        output = capsys.readouterr().out
        assert "Usage:" in output

    @pytest.mark.parametrize(
        "argv",
        [
            ["journal service", "--help"],
            ["journal service", "-h"],
            ["journal up", "up", "--help"],
            ["journal down", "down", "--help"],
        ],
    )
    def test_help_exits_without_lifecycle(self, monkeypatch, capsys, argv):
        monkeypatch.setattr(sys, "argv", argv)
        monkeypatch.setattr(
            service, "_up", lambda **_kwargs: pytest.fail("should not run lifecycle")
        )
        monkeypatch.setattr(
            service, "_down", lambda **_kwargs: pytest.fail("should not run lifecycle")
        )

        with pytest.raises(SystemExit) as exc:
            service.main()

        assert exc.value.code == 0
        output = capsys.readouterr().out
        assert "Usage:" in output

    def test_unknown_subcommand(self, monkeypatch, capsys):
        monkeypatch.setattr(sys, "argv", ["journal service", "bogus"])
        with pytest.raises(SystemExit):
            service.main()
        assert "Unknown subcommand" in capsys.readouterr().err

    def test_restart_if_installed_flag(self, monkeypatch):
        monkeypatch.setattr(
            sys, "argv", ["journal service", "restart", "--if-installed"]
        )
        with patch("solstone.think.service._restart", return_value=0) as mock:
            with pytest.raises(SystemExit):
                service.main()
            mock.assert_called_once_with(if_installed=True)


class TestRemoveStalePlists:
    @staticmethod
    def _configure(monkeypatch, tmp_path, *, platform="darwin", current=None):
        launch_agents = tmp_path / "LaunchAgents"
        plist_path = launch_agents / "org.solpbc.solstone.plist"
        current_path = current or (tmp_path / "current" / ".venv" / "bin" / "sol")
        monkeypatch.setattr(sys, "platform", platform)
        monkeypatch.setattr(service, "_plist_path", lambda: plist_path)
        monkeypatch.setattr(
            service,
            "_managed_wrapper",
            lambda binary: str(Path(current_path).with_name(binary)),
        )
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        return launch_agents, plist_path, Path(current_path)

    @staticmethod
    def _write_plist(path, *, label, program_arguments=None, program=None):
        path.parent.mkdir(parents=True, exist_ok=True)
        data = {"Label": label}
        if program_arguments is not None:
            data["ProgramArguments"] = program_arguments
        if program is not None:
            data["Program"] = program
        path.write_bytes(plistlib.dumps(data))

    @staticmethod
    def _touch(path):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")

    def test_removes_stale_plist_from_old_checkout(self, monkeypatch, tmp_path, capsys):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        old = tmp_path / "old" / ".venv" / "bin" / "sol"
        self._touch(old)
        self._touch(current)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(old), "supervisor", "5015"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)

        run.assert_called_once_with(
            ["launchctl", "bootout", "gui/501", str(plist_path)],
            capture_output=True,
            text=True,
        )
        assert not plist_path.exists()
        captured = capsys.readouterr()
        assert str(old) in captured.out
        assert str(current) in captured.out
        assert captured.err == ""

    def test_preserves_current_plist(self, monkeypatch, tmp_path, capsys):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(current), "supervisor", "5015"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        assert plist_path.exists()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_preserves_current_journal_plist(self, monkeypatch, tmp_path, capsys):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        current_journal = current.with_name("journal")
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current_journal)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(current_journal), "supervisor", "5015"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        assert plist_path.exists()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_removes_stale_journal_plist(self, monkeypatch, tmp_path, capsys):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        old = tmp_path / "old" / ".venv" / "bin" / "journal"
        self._touch(old)
        self._touch(current.with_name("journal"))
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(old), "supervisor", "5015"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)

        run.assert_called_once_with(
            ["launchctl", "bootout", "gui/501", str(plist_path)],
            capture_output=True,
            text=True,
        )
        assert not plist_path.exists()
        captured = capsys.readouterr()
        assert str(old) in captured.out
        assert captured.err == ""

    def test_preserves_current_plist_with_symlinked_venv_path(
        self, monkeypatch, tmp_path, capsys
    ):
        real_venv = tmp_path / "real_venv"
        linked_venv = tmp_path / "linked_venv"
        current = linked_venv / "bin" / "sol"
        launch_agents, plist_path, current = self._configure(
            monkeypatch, tmp_path, current=current
        )
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(real_venv / "bin" / "sol")
        linked_venv.symlink_to(real_venv, target_is_directory=True)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(current), "supervisor", "5015"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_removes_multiple_stale_plists_and_logs_only_unexpected_bootout_stderr(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        old_one = tmp_path / "old-one" / ".venv" / "bin" / "sol"
        old_two = tmp_path / "old-two" / ".venv" / "bin" / "sol"
        self._touch(old_one)
        self._touch(old_two)
        dev_path = launch_agents / "org.solpbc.solstone.dev.plist"
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(old_one)],
        )
        self._write_plist(
            dev_path,
            label=f"{service.SERVICE_LABEL}.dev",
            program_arguments=[str(old_two)],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.side_effect = [
                MagicMock(returncode=1, stderr="Could not find service"),
                MagicMock(returncode=1, stderr="unexpected doom"),
            ]
            assert service.remove_stale_plists() == (2, 0)

        assert run.call_count == 2
        assert not plist_path.exists()
        assert not dev_path.exists()
        captured = capsys.readouterr()
        assert "unexpected doom" in captured.err
        assert "Could not find service" not in captured.err
        assert captured.out.count("Removed stale launchd plist") == 2

    def test_counts_unlink_failure_without_aborting(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        old_one = tmp_path / "old-one" / ".venv" / "bin" / "sol"
        old_two = tmp_path / "old-two" / ".venv" / "bin" / "sol"
        self._touch(old_one)
        self._touch(old_two)
        dev_path = launch_agents / "org.solpbc.solstone.dev.plist"
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(old_one)],
        )
        self._write_plist(
            dev_path,
            label=f"{service.SERVICE_LABEL}.dev",
            program_arguments=[str(old_two)],
        )

        original_unlink = Path.unlink

        def fake_unlink(path, *args, **kwargs):
            if path == dev_path:
                raise PermissionError("no permission")
            return original_unlink(path, *args, **kwargs)

        with (
            patch("solstone.think.service.subprocess.run") as run,
            patch.object(
                Path,
                "unlink",
                autospec=True,
                side_effect=fake_unlink,
            ),
        ):
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 1)

        assert run.call_count == 2
        captured = capsys.readouterr()
        assert "Removed stale launchd plist" in captured.out
        assert f"failed to remove {dev_path}" in captured.err

    def test_empty_launch_agents_dir_is_noop(self, monkeypatch, tmp_path, capsys):
        launch_agents, _plist_path, _current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_ignores_non_solstone_labels(self, monkeypatch, tmp_path, capsys):
        launch_agents, _plist_path, _current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._write_plist(
            launch_agents / "com.apple.foo.plist",
            label="com.apple.foo",
            program_arguments=["/tmp/sol"],
        )
        self._write_plist(
            launch_agents / "app.solstone.observer.plist",
            label="app.solstone.observer",
            program_arguments=["/tmp/sol"],
        )
        self._write_plist(
            launch_agents / "org.solpbc.solstone-swift.plist",
            label="org.solpbc.solstone-swift",
            program_arguments=["/tmp/sol"],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_skips_unparseable_plist(self, monkeypatch, tmp_path, capsys):
        launch_agents, _plist_path, _current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        bad_path = launch_agents / "broken.plist"
        bad_path.write_bytes(b"not a plist")

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert f"skipping {bad_path}" in captured.err
        assert captured.out == ""

    def test_is_idempotent_after_removal(self, monkeypatch, tmp_path, capsys):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        old = tmp_path / "old" / ".venv" / "bin" / "sol"
        self._touch(old)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(old)],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)
            first = capsys.readouterr()
            assert "Removed stale launchd plist" in first.out

            assert service.remove_stale_plists() == (0, 0)
            second = capsys.readouterr()

        assert run.call_count == 1
        assert second.out == ""
        assert second.err == ""

    def test_uses_program_key_when_program_arguments_missing(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        old = tmp_path / "old" / ".venv" / "bin" / "sol"
        self._touch(old)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program=str(old),
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)

        run.assert_called_once_with(
            ["launchctl", "bootout", "gui/501", str(plist_path)],
            capture_output=True,
            text=True,
        )
        captured = capsys.readouterr()
        assert str(old) in captured.out

    def test_skips_matching_label_without_program_fields(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, _current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._write_plist(plist_path, label=service.SERVICE_LABEL)

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert f"skipping {plist_path}: no Program or ProgramArguments" in captured.err
        assert captured.out == ""

    def test_removes_plist_when_referenced_binary_is_missing(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        missing = tmp_path / "missing" / ".venv" / "bin" / "sol"
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(missing)],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)

        run.assert_called_once()
        assert not plist_path.exists()
        captured = capsys.readouterr()
        assert str(missing) in captured.out

    def test_removes_plist_when_referenced_binary_is_broken_symlink(
        self, monkeypatch, tmp_path, capsys
    ):
        launch_agents, plist_path, current = self._configure(monkeypatch, tmp_path)
        launch_agents.mkdir(parents=True, exist_ok=True)
        self._touch(current)
        target = tmp_path / "gone-target"
        broken = tmp_path / "broken" / ".venv" / "bin" / "sol"
        broken.parent.mkdir(parents=True, exist_ok=True)
        broken.symlink_to(target)
        self._write_plist(
            plist_path,
            label=service.SERVICE_LABEL,
            program_arguments=[str(broken)],
        )

        with patch("solstone.think.service.subprocess.run") as run:
            run.return_value = MagicMock(returncode=0, stderr="", stdout="")
            assert service.remove_stale_plists() == (1, 0)

        run.assert_called_once()
        assert not plist_path.exists()
        captured = capsys.readouterr()
        assert str(broken) in captured.out

    def test_absent_launch_agents_dir_is_noop(self, monkeypatch, tmp_path, capsys):
        _launch_agents, _plist_path, _current = self._configure(monkeypatch, tmp_path)

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_linux_is_noop(self, monkeypatch, tmp_path, capsys):
        _launch_agents, _plist_path, _current = self._configure(
            monkeypatch, tmp_path, platform="linux"
        )

        with patch("solstone.think.service.subprocess.run") as run:
            assert service.remove_stale_plists() == (0, 0)

        run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""
