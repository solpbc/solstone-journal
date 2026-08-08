# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think/service.py - cross-platform service management."""

from __future__ import annotations

import json
import logging
import os
import plistlib
import subprocess
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.think import service

LAUNCHCTL_RUNNING_WITH_PID = """gui/501/org.solpbc.solstone = {
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

LAUNCHCTL_LOADED_NO_PID = """gui/501/org.solpbc.solstone = {
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


def _install_fake_launchd_clock(monkeypatch):
    fake = [0.0]
    monkeypatch.setattr(service.time, "monotonic", lambda: fake[0])
    monkeypatch.setattr(
        service.time,
        "sleep",
        lambda seconds: fake.__setitem__(0, fake[0] + seconds),
    )
    return fake


class _FakeUids:
    def __init__(self, real: int):
        self.real = real


class _FakeProcess:
    def __init__(
        self,
        *,
        pid: int = 99,
        uid: int = 501,
        name: str = "other",
        exe: str = "/Applications/Other.app/Contents/MacOS/other",
        uid_exc: Exception | None = None,
        exe_exc: Exception | None = None,
    ):
        self.pid = pid
        self._uid = uid
        self._name = name
        self._exe = exe
        self._uid_exc = uid_exc
        self._exe_exc = exe_exc
        self.exe_called = False

    def name(self) -> str:
        return self._name

    def uids(self) -> _FakeUids:
        if self._uid_exc:
            raise self._uid_exc
        return _FakeUids(self._uid)

    def exe(self) -> str:
        self.exe_called = True
        if self._exe_exc:
            raise self._exe_exc
        return self._exe


def _patch_supervisor_conflict_inspection(
    monkeypatch,
    tmp_path: Path,
    *,
    launchctl_returncode: int = 1,
    launchctl_stdout: str = "",
    launchctl_stderr: str = "service not found",
    processes: list[_FakeProcess] | None = None,
) -> Path:
    monkeypatch.setattr(sys, "platform", "darwin")
    monkeypatch.setattr(service.os, "getuid", lambda: 501)
    plist_path = tmp_path / "org.solpbc.solstone.plist"
    monkeypatch.setattr(service, "_plist_path", lambda: plist_path)
    monkeypatch.setattr(
        service.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            args=args[0],
            returncode=launchctl_returncode,
            stdout=launchctl_stdout,
            stderr=launchctl_stderr,
        ),
    )
    monkeypatch.setattr(
        service.psutil,
        "process_iter",
        lambda: [] if processes is None else processes,
    )
    return plist_path


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
        assert plist["ProgramArguments"][1] == "start"
        assert plist["EnvironmentVariables"] == env
        assert plist["EnvironmentVariables"]["PYTHONUNBUFFERED"] == "1"
        assert plist["KeepAlive"] == {"SuccessfulExit": False}
        assert plist["SoftResourceLimits"] == {"NumberOfFiles": 4096}
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
        assert "TimeoutStartSec=120" in unit
        assert "Restart=on-failure" in unit
        assert "StartLimitIntervalSec=120" in unit
        assert "StartLimitBurst=10" in unit
        assert "KillMode=control-group" in unit
        assert "TimeoutStopSec=30" in unit
        assert "LimitNOFILE=4096" in unit
        assert f"StandardOutput=append:{service_log}" in unit
        assert f"StandardError=append:{service_log}" in unit
        assert "StandardError=inherit" not in unit
        assert (
            f"ExecStart={Path.home() / '.local' / 'bin' / 'journal'} start 5015" in unit
        )
        assert "start" in unit
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
        assert f"StandardError=append:{service_log}" in unit
        assert "StandardError=inherit" not in unit

    def test_invalid_journal_path_rejected(self):
        with pytest.raises(ValueError, match="shell-active character"):
            service._generate_systemd_unit({}, journal_path="/tmp/bad$path")

    def test_ready_timeout_ordering_invariant(self, tmp_path):
        from solstone.think import supervisor

        assert (
            service.READY_TIMEOUT_SECONDS
            >= supervisor.CONVEY_READY_WINDOW_SECONDS
            >= supervisor.REALISTIC_COLD_BIND_SECONDS
        )

        unit = service._generate_systemd_unit(
            {
                "HOME": "/home/test",
                "PATH": "/usr/bin",
            },
            journal_path=str(tmp_path / "journal"),
        )
        timeout_line = next(
            line for line in unit.splitlines() if line.startswith("TimeoutStartSec=")
        )
        timeout = int(timeout_line.split("=", 1)[1])

        assert timeout >= supervisor.CONVEY_READY_WINDOW_SECONDS
        assert timeout == service.SERVICE_START_TIMEOUT_SECONDS


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
        run_mock = MagicMock(
            return_value=MagicMock(returncode=0, stdout=LAUNCHCTL_RUNNING_WITH_PID)
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
        run_mock = MagicMock(
            return_value=MagicMock(returncode=0, stdout=LAUNCHCTL_LOADED_NO_PID)
        )
        monkeypatch.setattr(service.subprocess, "run", run_mock)
        assert service.service_is_running() is False


class TestSupervisorConflictInspection:
    def test_non_darwin_guard_does_not_probe(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(
            service.subprocess,
            "run",
            lambda *args, **kwargs: pytest.fail("launchctl should not be called"),
        )
        monkeypatch.setattr(
            service.psutil,
            "process_iter",
            lambda: pytest.fail("processes should not be enumerated"),
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "absent"
        assert evidence.label_state == "unloaded"
        assert evidence.app_state == "absent"
        assert not evidence.is_conflict
        assert "not applicable" in evidence.detail

    def test_launchctl_with_pid_is_loaded(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(
            monkeypatch,
            tmp_path,
            launchctl_returncode=0,
            launchctl_stdout=LAUNCHCTL_RUNNING_WITH_PID,
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "loaded"
        assert evidence.label_pid == 12345

    def test_launchctl_without_pid_is_loaded(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(
            monkeypatch,
            tmp_path,
            launchctl_returncode=0,
            launchctl_stdout=LAUNCHCTL_LOADED_NO_PID,
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "loaded"
        assert evidence.label_pid is None

    def test_launchctl_recognized_not_loaded_marker_is_unloaded(
        self, monkeypatch, tmp_path
    ):
        _patch_supervisor_conflict_inspection(
            monkeypatch,
            tmp_path,
            launchctl_returncode=113,
            launchctl_stderr="Bootstrap lookup failed: service not found",
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "unloaded"

    def test_launchctl_opaque_nonzero_is_unknown(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(
            monkeypatch,
            tmp_path,
            launchctl_returncode=5,
            launchctl_stderr="launchctl returned an opaque error",
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "unknown"

    def test_launchctl_timeout_is_unknown(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)

        def timeout(command, **kwargs):
            raise subprocess.TimeoutExpired(command, kwargs["timeout"])

        monkeypatch.setattr(service.subprocess, "run", timeout)

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "unknown"

    def test_launchctl_oserror_is_unknown(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        monkeypatch.setattr(
            service.subprocess,
            "run",
            lambda *args, **kwargs: (_ for _ in ()).throw(OSError("launchctl failed")),
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.label_state == "unknown"

    def test_process_iter_failure_is_app_unknown(self, monkeypatch, tmp_path):
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        monkeypatch.setattr(
            service.psutil,
            "process_iter",
            lambda: (_ for _ in ()).throw(service.psutil.Error("boom")),
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "unknown"

    def test_process_uid_filter_runs_before_exe(self, monkeypatch, tmp_path):
        other_user_proc = _FakeProcess(
            uid=0,
            exe_exc=service.psutil.AccessDenied(pid=1),
        )
        _patch_supervisor_conflict_inspection(
            monkeypatch,
            tmp_path,
            processes=[other_user_proc],
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "absent"
        assert other_user_proc.exe_called is False

    def test_current_uid_access_denied_exe_taints_app_unknown(
        self, monkeypatch, tmp_path
    ):
        proc = _FakeProcess(
            uid=501,
            exe_exc=service.psutil.AccessDenied(pid=2),
        )
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path, processes=[proc])

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "unknown"

    def test_process_races_do_not_taint_app_state(self, monkeypatch, tmp_path):
        procs = [
            _FakeProcess(uid_exc=service.psutil.NoSuchProcess(pid=1)),
            _FakeProcess(uid_exc=service.psutil.ZombieProcess(pid=2)),
        ]
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path, processes=procs)

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "absent"

    def test_app_running_matches_executable_suffix(self, monkeypatch, tmp_path):
        executable = (
            "/private/var/folders/xx/AppTranslocation/ABC/d/"
            "journal.app/Contents/MacOS/journal"
        )
        proc = _FakeProcess(
            pid=2468,
            uid=501,
            exe=executable,
        )
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path, processes=[proc])

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "running"
        assert evidence.app_pid == 2468
        assert evidence.app_executable == executable
        assert executable in evidence.detail

    def test_supervisor_process_title_does_not_match_app(self, monkeypatch, tmp_path):
        proc = _FakeProcess(
            uid=501,
            name="journal:supervisor",
            exe="/home/x/.venv/bin/python3.12",
        )
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path, processes=[proc])

        evidence = service.inspect_supervisor_conflict()

        assert proc.name() == "journal:supervisor"
        assert evidence.app_state == "absent"

    def test_matching_app_under_other_uid_does_not_match(self, monkeypatch, tmp_path):
        proc = _FakeProcess(
            uid=0,
            exe="/Applications/journal.app/Contents/MacOS/journal",
        )
        _patch_supervisor_conflict_inspection(monkeypatch, tmp_path, processes=[proc])

        evidence = service.inspect_supervisor_conflict()

        assert evidence.app_state == "absent"

    def test_plist_parse_failure_is_malformed(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        plist_path.write_text("not a plist", encoding="utf-8")

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "malformed"
        assert evidence.foreign.incomplete_paths == (str(plist_path),)

    def test_plist_missing_argv_is_malformed(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        plist_path.write_bytes(plistlib.dumps({"Label": service.SERVICE_LABEL}))

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "malformed"

    def test_plist_wrong_label_is_still_present(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        plist_path.write_bytes(
            plistlib.dumps(
                {"Label": "wrong.label", "ProgramArguments": ["/tmp/missing", "start"]}
            )
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "present"

    def test_plist_missing_label_is_still_present(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        plist_path.write_bytes(
            plistlib.dumps({"ProgramArguments": ["/tmp/missing", "start"]})
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "present"

    def test_plist_missing_executable_is_still_present(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        plist_path.write_bytes(
            plistlib.dumps(
                {
                    "Label": service.SERVICE_LABEL,
                    "ProgramArguments": ["/tmp/definitely-missing", "start"],
                }
            )
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "present"

    def test_stat_permission_error_is_plist_unknown(self, monkeypatch, tmp_path):
        plist_path = _patch_supervisor_conflict_inspection(monkeypatch, tmp_path)
        original_stat = service.os.stat

        def deny_plist_stat(path, *args, **kwargs):
            if Path(path) == plist_path:
                raise PermissionError("denied")
            return original_stat(path, *args, **kwargs)

        monkeypatch.setattr(
            service.os,
            "stat",
            deny_plist_stat,
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.plist_state == "unknown"


class TestForeignSolstoneAppLaunchers:
    @staticmethod
    def _set_scan_dir(monkeypatch, launch_agents: Path) -> None:
        monkeypatch.setattr(sys, "platform", "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: launch_agents / "org.solpbc.solstone.plist",
        )

    @staticmethod
    def _configure(monkeypatch, tmp_path: Path) -> Path:
        launch_agents = tmp_path / "LaunchAgents"
        TestForeignSolstoneAppLaunchers._set_scan_dir(monkeypatch, launch_agents)
        launch_agents.mkdir(parents=True, exist_ok=True)
        return launch_agents

    @staticmethod
    def _write_plist(path: Path, data: object) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(plistlib.dumps(data))

    @staticmethod
    def _skip_if_root_for_chmod() -> None:
        if os.geteuid() == 0:
            pytest.skip("chmod-based permission checks are ineffective as root")

    def test_load_launchd_plist_returns_dict_or_none(self, tmp_path):
        valid = tmp_path / "valid.plist"
        invalid = tmp_path / "invalid.plist"
        list_payload = tmp_path / "list.plist"
        self._write_plist(valid, {"Label": "x"})
        invalid.write_text("not a plist", encoding="utf-8")
        self._write_plist(list_payload, ["not", "a", "dict"])

        assert service._load_launchd_plist(valid) == {"Label": "x"}
        assert service._load_launchd_plist(invalid) is None
        assert service._load_launchd_plist(list_payload) is None
        assert service._load_launchd_plist(tmp_path / "missing.plist") is None

    @pytest.mark.parametrize(
        ("candidate", "expected"),
        [
            ("/Applications/solstone.app", True),
            ("/Applications/solstone.app/Contents/MacOS/solstone", True),
            ('open -a "/Applications/solstone.app"', True),
            ("exec /Applications/solstone.app; sleep 1", True),
            ("/tmp/Applications/solstone.app", False),
            ("/Applications/solstone.application", False),
            ("/Applications/solstone.app.old", False),
            ("/Applications/solstone.app2", False),
        ],
    )
    def test_app_path_boundary_matrix(self, candidate, expected):
        assert service._mentions_solstone_app_bundle(candidate) is expected

    @pytest.mark.parametrize(
        ("keep_alive", "expected"),
        [
            (True, True),
            ({"SuccessfulExit": False}, True),
            ({"Crashed": True}, True),
            (False, False),
            ({}, False),
            ("true", False),
            (1, False),
            (None, False),
        ],
    )
    def test_keepalive_persistence_matrix(
        self, monkeypatch, tmp_path, keep_alive, expected
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        data = {
            "Label": "com.example.solstone-watchdog",
            "ProgramArguments": ["/usr/bin/open", "-a", "/Applications/solstone.app"],
        }
        if keep_alive is not None:
            data["KeepAlive"] = keep_alive
        self._write_plist(launch_agents / "foreign.plist", data)

        scan = service._scan_foreign_solstone_app_launchers()

        assert bool(scan.matches) is expected
        assert scan.incomplete_paths == ()

    def test_foreign_launcher_incident_regression_direct_program(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "com.example.solstone-watchdog.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.solstone-watchdog",
                "Program": "/Applications/solstone.app/Contents/MacOS/solstone",
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.incomplete_paths == ()
        assert scan.matches == (
            service.ForeignLauncherMatch(
                label="com.example.solstone-watchdog",
                plist_path=str(plist_path),
                service_target="gui/501/com.example.solstone-watchdog",
            ),
        )

    def test_foreign_launcher_matches_open_and_shell_arguments(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        self._write_plist(
            launch_agents / "open.plist",
            {
                "Label": "com.example.open",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )
        self._write_plist(
            launch_agents / "shell.plist",
            {
                "Label": "com.example.shell",
                "ProgramArguments": [
                    "/bin/sh",
                    "-c",
                    "exec /usr/bin/open -a /Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert [match.label for match in scan.matches] == [
            "com.example.open",
            "com.example.shell",
        ]

    def test_foreign_launcher_ignores_non_command_fields(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        self._write_plist(
            launch_agents / "non-command.plist",
            {
                "Label": "com.example.solstone-watchdog",
                "BundleProgram": "/Applications/solstone.app",
                "EnvironmentVariables": {"TARGET": "/Applications/solstone.app"},
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == ()

    @pytest.mark.parametrize(
        "label",
        [
            service.SERVICE_LABEL,
            f"{service.SERVICE_LABEL}.dev",
        ],
    )
    def test_product_labels_do_not_match(self, monkeypatch, tmp_path, label):
        launch_agents = self._configure(monkeypatch, tmp_path)
        self._write_plist(
            launch_agents / "product.plist",
            {
                "Label": label,
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == ()

    @pytest.mark.parametrize(
        "unsafe_char",
        ["\u0085", "\u2028", "\u2029", "\u202e"],
    )
    def test_unsafe_unicode_label_is_incomplete_and_detail_safe(
        self, monkeypatch, tmp_path, unsafe_char
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / f"unsafe-label-{ord(unsafe_char):x}.plist"
        self._write_plist(
            plist_path,
            {
                "Label": f"com.example.bad{unsafe_char}label",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()
        detail = service._supervisor_conflict_detail(
            launch_agents / "org.solpbc.solstone.plist",
            "absent",
            "unloaded",
            None,
            "absent",
            None,
            None,
            scan,
        )

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)
        assert unsafe_char not in detail

    def test_surrogateescape_filename_is_incomplete_and_detail_safe(
        self, monkeypatch, tmp_path
    ):
        if not sys.platform.startswith("linux"):
            pytest.skip("surrogateescape filename creation is only exercised on Linux")
        launch_agents = self._configure(monkeypatch, tmp_path)
        filename = os.fsdecode(b"bad\xffname.plist")
        plist_path = launch_agents / filename
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.solstone-watchdog",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()
        detail = service._supervisor_conflict_detail(
            launch_agents / "org.solpbc.solstone.plist",
            "absent",
            "unloaded",
            None,
            "absent",
            None,
            None,
            scan,
        )

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)
        assert "\udcff" not in detail
        assert "bad?name.plist" in detail

    def test_supervisor_conflict_detail_sanitizes_static_prefix(self):
        unsafe_chars = ("\u0085", "\u2028", "\u2029", "\u202e")

        detail = service._supervisor_conflict_detail(
            Path(f"/tmp/plist{unsafe_chars[3]}.plist"),
            "present",
            "loaded",
            12345,
            "running",
            2468,
            f"/tmp/journal{unsafe_chars[0]}.app/Contents/MacOS/journal",
            service.ForeignLauncherScan(
                matches=(
                    service.ForeignLauncherMatch(
                        label=f"com.example.bad{unsafe_chars[1]}label",
                        plist_path=f"/tmp/foreign{unsafe_chars[2]}.plist",
                        service_target="gui/501/com.example.bad",
                    ),
                ),
                incomplete_paths=(f"/tmp/incomplete{unsafe_chars[0]}.plist",),
            ),
        )

        assert not any(char in detail for char in unsafe_chars)
        assert "?" in detail

    def test_control_character_label_is_incomplete_not_match(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "control-label.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.bad\nlabel",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)

    def test_control_character_path_is_incomplete_and_detail_safe(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "bad\npath.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.solstone-watchdog",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()
        detail = service._supervisor_conflict_detail(
            launch_agents / "org.solpbc.solstone.plist",
            "absent",
            "unloaded",
            None,
            "absent",
            None,
            None,
            scan,
        )

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)
        assert "\n" not in detail
        assert "bad?path.plist" in detail

    def test_multi_field_same_plist_dedupes_to_one_match(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "multi.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.multi",
                "Program": "/Applications/solstone.app/Contents/MacOS/solstone",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == (
            service.ForeignLauncherMatch(
                label="com.example.multi",
                plist_path=str(plist_path),
                service_target="gui/501/com.example.multi",
            ),
        )

    def test_two_paths_one_label_sorted_by_label_then_path(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        later = launch_agents / "z.plist"
        earlier = launch_agents / "a.plist"
        for path in (later, earlier):
            self._write_plist(
                path,
                {
                    "Label": "com.example.duplicate",
                    "ProgramArguments": [
                        "/usr/bin/open",
                        "-a",
                        "/Applications/solstone.app",
                    ],
                    "KeepAlive": True,
                },
            )

        scan = service._scan_foreign_solstone_app_launchers()

        assert [match.plist_path for match in scan.matches] == [
            str(earlier),
            str(later),
        ]

    def test_incomplete_file_parse_failure(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        bad_path = launch_agents / "broken.plist"
        bad_path.write_text("not a plist", encoding="utf-8")

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(bad_path),)

    def test_incomplete_directory_unreadable(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        self._skip_if_root_for_chmod()

        try:
            launch_agents.chmod(0o000)
            scan = service._scan_foreign_solstone_app_launchers()
        finally:
            launch_agents.chmod(0o700)

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(launch_agents),)

    def test_incomplete_scan_path_regular_file(self, monkeypatch, tmp_path):
        launch_agents = tmp_path / "LaunchAgents"
        launch_agents.write_text("not a directory", encoding="utf-8")
        self._set_scan_dir(monkeypatch, launch_agents)

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(launch_agents),)

    def test_incomplete_scan_path_dangling_symlink(self, monkeypatch, tmp_path):
        launch_agents = tmp_path / "LaunchAgents"
        launch_agents.symlink_to(tmp_path / "missing-target", target_is_directory=True)
        self._set_scan_dir(monkeypatch, launch_agents)

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(launch_agents),)

    def test_missing_launch_agents_dir_is_complete_empty(self, monkeypatch, tmp_path):
        launch_agents = tmp_path / "missing" / "LaunchAgents"
        self._set_scan_dir(monkeypatch, launch_agents)

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == ()

    def test_incomplete_scan_path_blocked_parent(self, monkeypatch, tmp_path):
        self._skip_if_root_for_chmod()
        blocked_parent = tmp_path / "blocked"
        launch_agents = blocked_parent / "LaunchAgents"
        launch_agents.mkdir(parents=True)
        self._set_scan_dir(monkeypatch, launch_agents)

        try:
            blocked_parent.chmod(0o000)
            scan = service._scan_foreign_solstone_app_launchers()
        finally:
            blocked_parent.chmod(0o700)

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(launch_agents),)

    def test_incomplete_scan_path_disappears_after_metadata(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        original_lstat = service.os.lstat

        def remove_after_lstat(path, *args, **kwargs):
            result = original_lstat(path, *args, **kwargs)
            if Path(path) == launch_agents:
                launch_agents.rmdir()
            return result

        # This seam only sequences a real race; Path.iterdir remains unmocked,
        # so the scanner observes the real FileNotFoundError.
        monkeypatch.setattr(service.os, "lstat", remove_after_lstat)

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(launch_agents),)

    def test_readable_plist_symlink_matches_normally(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        target = tmp_path / "target.plist"
        self._write_plist(
            target,
            {
                "Label": "com.example.symlink",
                "ProgramArguments": [
                    "/usr/bin/open",
                    "-a",
                    "/Applications/solstone.app",
                ],
                "KeepAlive": True,
            },
        )
        plist_path = launch_agents / "link.plist"
        plist_path.symlink_to(target)

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.incomplete_paths == ()
        assert scan.matches == (
            service.ForeignLauncherMatch(
                label="com.example.symlink",
                plist_path=str(plist_path),
                service_target="gui/501/com.example.symlink",
            ),
        )

    def test_broken_plist_symlink_is_incomplete(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "broken-link.plist"
        plist_path.symlink_to(tmp_path / "missing.plist")

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)

    def test_non_string_program_is_incomplete_not_match(self, monkeypatch, tmp_path):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "bad-program.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.bad-program",
                "Program": {"target": "/Applications/solstone.app"},
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)

    def test_program_arguments_not_list_is_incomplete_not_match(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "bad-arguments.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.bad-arguments",
                "ProgramArguments": "/Applications/solstone.app",
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)

    def test_program_arguments_non_string_element_does_not_coerce_to_match(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "coercion-regression.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.coercion",
                "ProgramArguments": [{"target": "/Applications/solstone.app"}],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == ()
        assert scan.incomplete_paths == (str(plist_path),)

    def test_valid_program_with_malformed_arguments_matches_and_is_incomplete(
        self, monkeypatch, tmp_path
    ):
        launch_agents = self._configure(monkeypatch, tmp_path)
        plist_path = launch_agents / "match-and-incomplete.plist"
        self._write_plist(
            plist_path,
            {
                "Label": "com.example.match-and-incomplete",
                "Program": "/Applications/solstone.app/Contents/MacOS/solstone",
                "ProgramArguments": [{"target": "/Applications/solstone.app"}],
                "KeepAlive": True,
            },
        )

        scan = service._scan_foreign_solstone_app_launchers()

        assert scan.matches == (
            service.ForeignLauncherMatch(
                label="com.example.match-and-incomplete",
                plist_path=str(plist_path),
                service_target="gui/501/com.example.match-and-incomplete",
            ),
        )
        assert scan.incomplete_paths == (str(plist_path),)

    def test_non_darwin_inspection_does_not_scan_foreign_launchers(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(
            service,
            "_scan_foreign_solstone_app_launchers",
            lambda: pytest.fail("foreign launchers should not be scanned"),
        )

        evidence = service.inspect_supervisor_conflict()

        assert evidence.foreign == service.ForeignLauncherScan((), ())


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
    @pytest.mark.parametrize("platform", ["darwin", "linux"])
    def test_up_errors_when_not_installed(self, monkeypatch, capsys, platform):
        monkeypatch.setattr(sys, "platform", platform)
        monkeypatch.setattr(service, "service_is_installed", lambda: False)
        install = MagicMock()
        start = MagicMock()
        clear_ready = MagicMock()
        wait_ready = MagicMock()
        status = MagicMock()
        monkeypatch.setattr(service, "_install", install)
        monkeypatch.setattr(service, "_start", start)
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)
        monkeypatch.setattr(service, "_status", status)

        assert service._up() == 1
        install.assert_not_called()
        start.assert_not_called()
        clear_ready.assert_not_called()
        wait_ready.assert_not_called()
        status.assert_not_called()
        err = capsys.readouterr().err
        assert "journal setup" in err
        assert "journal service install" in err

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

        assert service._up() == 0
        start.assert_called_once_with()
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_called_once_with()

    def test_up_accepts_readiness_when_start_reports_race(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "service_is_running", lambda: False)
        start = MagicMock(return_value=1)
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value={"pid": 123})
        status = MagicMock(return_value=0)
        monkeypatch.setattr(service, "_start", start)
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)
        monkeypatch.setattr(service, "_status", status)

        assert service._up() == 0
        start.assert_called_once_with()
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_called_once_with()

    def test_up_preserves_start_failure_without_readiness(self, monkeypatch):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service, "service_is_installed", lambda: True)
        monkeypatch.setattr(service, "service_is_running", lambda: False)
        start = MagicMock(return_value=7)
        clear_ready = MagicMock()
        wait_ready = MagicMock(return_value=None)
        status = MagicMock(return_value=0)
        monkeypatch.setattr(service, "_start", start)
        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr(service, "wait_ready", wait_ready)
        monkeypatch.setattr(service, "_status", status)

        assert service._up() == 7
        start.assert_called_once_with()
        clear_ready.assert_called_once_with()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_not_called()

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

        assert service._up() == 0
        clear_ready.assert_not_called()
        wait_ready.assert_called_once_with(timeout=service.READY_TIMEOUT_SECONDS)
        status.assert_called_once_with()


class TestInstall:
    def test_darwin_clears_readiness_before_bootstrap(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        calls = []
        commands = []

        def clear_ready():
            calls.append("clear_ready")

        def run(command, **kwargs):
            commands.append((command, kwargs))
            if command[:2] == ["launchctl", "print"]:
                return subprocess.CompletedProcess(
                    args=command, returncode=1, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                calls.append("bootstrap")
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr(service, "clear_ready", clear_ready)
        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert calls.index("clear_ready") < calls.index("bootstrap")
        assert commands[0] == (
            ["launchctl", "bootout", f"gui/501/{service.SERVICE_LABEL}"],
            {"capture_output": True, "check": False},
        )

    def test_darwin_waits_for_label_unload_before_bootstrap(
        self, monkeypatch, tmp_path
    ):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        _install_fake_launchd_clock(monkeypatch)
        events = []
        poll_count = 0

        def run(command, **kwargs):
            nonlocal poll_count
            if command[:2] == ["launchctl", "bootout"]:
                events.append("bootout")
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "print"]:
                poll_count += 1
                if poll_count == 1:
                    events.append("print-present")
                    return subprocess.CompletedProcess(
                        args=command, returncode=0, stdout="", stderr=""
                    )
                events.append("print-absent")
                return subprocess.CompletedProcess(
                    args=command, returncode=1, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                events.append("bootstrap")
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert events.index("bootout") < events.index("print-present")
        assert events.index("print-present") < events.index("print-absent")
        assert events.index("print-absent") < events.index("bootstrap")

    def test_darwin_install_fails_when_label_never_unloads(
        self, monkeypatch, tmp_path, caplog, capsys
    ):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        _install_fake_launchd_clock(monkeypatch)
        monkeypatch.setattr(service, "_LAUNCHD_UNLOAD_TIMEOUT_S", 0.5)
        bootstrap_count = 0

        def run(command, **kwargs):
            nonlocal bootstrap_count
            if command[:2] == ["launchctl", "print"]:
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                bootstrap_count += 1
                return subprocess.CompletedProcess(
                    args=command,
                    returncode=1,
                    stdout="",
                    stderr="Bootstrap failed: 5: Input/output error",
                )
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        with caplog.at_level(logging.WARNING):
            assert service._install() == 1

        assert bootstrap_count == service._LAUNCHD_BOOTSTRAP_MAX_ATTEMPTS
        assert "did not unload" in caplog.text
        stderr = capsys.readouterr().err
        assert "Error loading service" in stderr
        assert "Bootstrap failed: 5: Input/output error" in stderr

    def test_darwin_slow_unload_clears_during_retry_succeeds(
        self, monkeypatch, tmp_path, capsys
    ):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        _install_fake_launchd_clock(monkeypatch)
        monkeypatch.setattr(service, "_LAUNCHD_UNLOAD_TIMEOUT_S", 0.5)
        bootstrap_count = 0

        def run(command, **kwargs):
            nonlocal bootstrap_count
            if command[:2] == ["launchctl", "print"]:
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                bootstrap_count += 1
                if bootstrap_count == 1:
                    return subprocess.CompletedProcess(
                        args=command,
                        returncode=1,
                        stdout="",
                        stderr="Bootstrap failed: 5: Input/output error",
                    )
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="", stderr=""
                )
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert bootstrap_count >= 2
        assert "Error loading service" not in capsys.readouterr().err

    def test_darwin_bootstrap_eio_retried_and_succeeds(
        self, monkeypatch, tmp_path, capsys
    ):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        _install_fake_launchd_clock(monkeypatch)
        sequence = []
        bootstrap_count = 0

        def run(command, **kwargs):
            nonlocal bootstrap_count
            sequence.append(command[:2])
            if command[:2] == ["launchctl", "print"]:
                return subprocess.CompletedProcess(
                    args=command, returncode=1, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                bootstrap_count += 1
                if bootstrap_count == 1:
                    return subprocess.CompletedProcess(
                        args=command,
                        returncode=1,
                        stdout="",
                        stderr="Bootstrap failed: 5: Input/output error",
                    )
                return subprocess.CompletedProcess(
                    args=command, returncode=0, stdout="", stderr=""
                )
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert bootstrap_count == 2
        bootstrap_indexes = [
            index
            for index, command in enumerate(sequence)
            if command == ["launchctl", "bootstrap"]
        ]
        reprobe_indexes = [
            index
            for index, command in enumerate(sequence)
            if command == ["launchctl", "print"]
        ]
        assert any(
            bootstrap_indexes[0] < index < bootstrap_indexes[1]
            for index in reprobe_indexes
        )
        assert "Error loading service" not in capsys.readouterr().err

    def test_darwin_noneio_bootstrap_failure_fails_fast(
        self, monkeypatch, tmp_path, caplog, capsys
    ):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        bootstrap_count = 0

        def run(command, **kwargs):
            nonlocal bootstrap_count
            if command[:2] == ["launchctl", "print"]:
                return subprocess.CompletedProcess(
                    args=command, returncode=1, stdout="", stderr=""
                )
            if command[:2] == ["launchctl", "bootstrap"]:
                bootstrap_count += 1
                return subprocess.CompletedProcess(
                    args=command,
                    returncode=1,
                    stdout="",
                    stderr="Bootstrap failed: 1: Operation not permitted",
                )
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        with caplog.at_level(logging.WARNING):
            assert service._install() == 1

        assert bootstrap_count == 1
        stderr = capsys.readouterr().err
        assert "Error loading service" in stderr
        assert "Operation not permitted" in stderr
        assert "did not unload" not in caplog.text

    def test_darwin_bootstrap_proceeds_when_probe_raises(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr(
            service,
            "_plist_path",
            lambda: tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist",
        )
        monkeypatch.setattr(service, "remove_stale_plists", MagicMock())
        events = []

        def run(command, **kwargs):
            if command[:2] == ["launchctl", "print"]:
                raise subprocess.TimeoutExpired(cmd=command, timeout=1.0)
            if command[:2] == ["launchctl", "bootstrap"]:
                events.append("bootstrap")
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        assert service._install() == 0
        assert "bootstrap" in events

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


class TestUninstall:
    def test_darwin_bootout_uses_label_form(self, monkeypatch, tmp_path):
        monkeypatch.setattr(service, "_platform", lambda: "darwin")
        monkeypatch.setattr(service.os, "getuid", lambda: 501)
        plist_path = tmp_path / "LaunchAgents" / "org.solpbc.solstone.plist"
        plist_path.parent.mkdir(parents=True)
        plist_path.write_text("", encoding="utf-8")
        monkeypatch.setattr(service, "_plist_path", lambda: plist_path)
        commands = []

        def run(command, **kwargs):
            commands.append((command, kwargs))
            return subprocess.CompletedProcess(
                args=command, returncode=0, stdout="", stderr=""
            )

        monkeypatch.setattr("subprocess.run", run)

        assert service._uninstall() == 0
        assert commands == [
            (
                ["launchctl", "bootout", f"gui/501/{service.SERVICE_LABEL}"],
                {"capture_output": True, "check": False},
            )
        ]


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
