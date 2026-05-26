# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


def _console_script(binary: str) -> Path:
    path = Path(sys.executable).parent / binary
    if not path.exists():
        pytest.skip(f"{path} is not installed")
    return path


@pytest.mark.parametrize(
    ("binary", "args", "usage"),
    [
        ("sol", ["heartbeat", "--help"], "usage: sol heartbeat"),
        ("journal", ["heartbeat", "--help"], "usage: journal heartbeat"),
        ("sol", ["setup", "--help"], "usage: sol setup"),
        ("journal", ["setup", "--help"], "usage: journal setup"),
        ("sol", ["config", "journal", "--help"], "usage: sol config journal"),
        (
            "journal",
            ["config", "journal", "--help"],
            "usage: journal config journal",
        ),
    ],
)
def test_nested_argparse_usage_uses_invoked_binary(
    binary: str,
    args: list[str],
    usage: str,
) -> None:
    env = os.environ.copy()
    env["SOLSTONE_JOURNAL"] = str(Path("tests/fixtures/journal").resolve())

    result = subprocess.run(
        [str(_console_script(binary)), *args],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )

    assert result.returncode == 0
    assert usage in result.stdout
