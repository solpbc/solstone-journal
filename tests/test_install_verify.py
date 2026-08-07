# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import ast
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
VENV_PYTHON = REPO_ROOT / ".venv/bin/python"
SOL_BIN = REPO_ROOT / ".venv/bin/sol"


def _run_in_tmp(args: list[str]) -> subprocess.CompletedProcess[str]:
    if not VENV_PYTHON.exists():
        pytest.skip("venv not installed")
    return subprocess.run(
        args,
        cwd="/tmp",
        timeout=30,
        capture_output=True,
        text=True,
    )


def test_import_think_utils_from_tmp():
    result = _run_in_tmp(
        [str(VENV_PYTHON), "-c", "from solstone.think.utils import get_journal"]
    )

    assert result.returncode == 0, result.stderr


def test_import_think_media_from_tmp():
    result = _run_in_tmp(
        [str(VENV_PYTHON), "-c", "from solstone.think.media import MIME_TYPES"]
    )

    assert result.returncode == 0, result.stderr


def test_sol_version_from_tmp():
    if not SOL_BIN.exists():
        pytest.skip("sol console script not installed")

    result = _run_in_tmp([str(SOL_BIN), "--version"])

    assert result.returncode == 0, result.stderr


def test_editable_finder_mapping_has_no_root_aliases():
    finder_paths = list(
        (REPO_ROOT / ".venv" / "lib").glob(
            "python*/site-packages/__editable___solstone_*_finder.py"
        )
    )
    if not finder_paths:
        pytest.skip("editable finder not installed")

    module = ast.parse(finder_paths[0].read_text(encoding="utf-8"))
    mapping = None
    for node in module.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id == "MAPPING":
                    mapping = ast.literal_eval(node.value)
                    break
        elif isinstance(node, ast.AnnAssign):
            if isinstance(node.target, ast.Name) and node.target.id == "MAPPING":
                mapping = ast.literal_eval(node.value)
        else:
            continue
        if mapping is not None:
            break

    assert mapping is not None, "MAPPING assignment not found"
    assert "sol" not in mapping and "media" not in mapping
