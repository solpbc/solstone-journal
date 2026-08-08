# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from solstone.think import sol_cli


def _executable(path: Path) -> Path:
    path.write_text("#!/bin/sh\n", encoding="utf-8")
    path.chmod(0o755)
    return path


def test_describe_command_uses_the_native_binary() -> None:
    assert sol_cli.COMMANDS["describe"] == sol_cli.Command(
        "solstone-core-describe", "service", native=True
    )


def test_resolve_describe_binary_prefers_the_installed_sibling(tmp_path, monkeypatch) -> None:
    executable = tmp_path / "python"
    sibling = _executable(tmp_path / "solstone-core-describe")
    override = _executable(tmp_path / "override")
    monkeypatch.setenv("SOLSTONE_DESCRIBE_BIN", str(override))

    assert sol_cli.describe_path_for_executable(str(executable)) == sibling
    assert sol_cli.resolve_describe_binary(str(executable)) == sibling


def test_resolve_describe_binary_uses_the_explicit_development_override(
    tmp_path, monkeypatch
) -> None:
    override = _executable(tmp_path / "override")
    monkeypatch.setenv("SOLSTONE_DESCRIBE_BIN", str(override))

    assert sol_cli.resolve_describe_binary(str(tmp_path / "python")) == override


def test_resolve_describe_binary_returns_none_when_no_executable_exists(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.delenv("SOLSTONE_DESCRIBE_BIN", raising=False)

    assert sol_cli.resolve_describe_binary(str(tmp_path / "python")) is None


def test_native_command_execs_the_resolved_binary(monkeypatch, tmp_path) -> None:
    binary = _executable(tmp_path / "solstone-core-describe")
    command = sol_cli.Command("solstone-core-describe", "service", native=True)
    monkeypatch.setitem(sol_cli.COMMANDS, "native-test", command)
    monkeypatch.setattr(sol_cli, "_guard_journal_coherence", lambda: None)
    monkeypatch.setattr(sol_cli.setproctitle, "setproctitle", lambda _title: None)
    monkeypatch.setattr(sol_cli, "resolve_describe_binary", lambda: binary)
    monkeypatch.setattr(sys, "argv", ["journal", "native-test", "video.webm", "-j", "2"])
    called: dict[str, object] = {}

    def fake_execv(path: str, args: list[str]) -> None:
        called["path"] = path
        called["args"] = args
        raise SystemExit(0)

    monkeypatch.setattr(os, "execv", fake_execv)

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.journal_main()

    assert exc_info.value.code == 0
    assert called == {
        "path": str(binary),
        "args": [str(binary), "video.webm", "-j", "2"],
    }


def test_native_command_missing_binary_exits_127(monkeypatch, capsys) -> None:
    command = sol_cli.Command("solstone-core-describe", "service", native=True)
    monkeypatch.setitem(sol_cli.COMMANDS, "native-test", command)
    monkeypatch.setattr(sol_cli, "_guard_journal_coherence", lambda: None)
    monkeypatch.setattr(sol_cli.setproctitle, "setproctitle", lambda _title: None)
    monkeypatch.setattr(sol_cli, "resolve_describe_binary", lambda: None)
    monkeypatch.setattr(sys, "argv", ["journal", "native-test", "video.webm"])

    with pytest.raises(SystemExit) as exc_info:
        sol_cli.journal_main()

    assert exc_info.value.code == 127
    assert "SOLSTONE_DESCRIBE_BIN" in capsys.readouterr().err
