# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from solstone.think.sol_cli import ALIASES, COMMANDS


def _console_script(binary: str) -> Path:
    path = Path(sys.executable).parent / binary
    if not path.exists():
        pytest.skip(f"{path} is not installed")
    return path


SERVICE_SURFACE = sorted(
    {
        *(name for name, command in COMMANDS.items() if command.surface == "service"),
        *(name for name, alias in ALIASES.items() if alias.surface == "service"),
    }
)


@pytest.mark.parametrize("binary", ["sol", "journal"])
@pytest.mark.parametrize("command", SERVICE_SURFACE)
def test_service_surface_help_works_without_deprecation(
    binary: str,
    command: str,
) -> None:
    env = os.environ.copy()
    env["SOLSTONE_JOURNAL"] = str(Path("tests/fixtures/journal").resolve())

    result = subprocess.run(
        [str(_console_script(binary)), command, "--help"],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )

    combined = f"{result.stdout}\n{result.stderr}"
    assert result.returncode == 0
    assert "usage:" in result.stdout.lower()
    assert "DeprecationWarning" not in combined
    assert "deprecat" not in combined.lower()
