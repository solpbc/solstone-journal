# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Canary for the fixture-leak detector in the project-root conftest.py."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

pytest_plugins = ["pytester"]
pytestmark = pytest.mark.xdist_group("fixture_leak_detector")

_ROOT_CONFTEST = Path(__file__).resolve().parent.parent / "conftest.py"


def _prime_git_repo(project_dir: Path) -> None:
    """Initialise a miniature git repo with one tracked file under tests/fixtures/."""
    subprocess.run(["git", "init", "-q"], cwd=project_dir, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.email=canary@example.invalid",
            "-c",
            "user.name=canary",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "init",
        ],
        cwd=project_dir,
        check=True,
    )
    fixtures_dir = project_dir / "tests" / "fixtures"
    fixtures_dir.mkdir(parents=True, exist_ok=True)
    (fixtures_dir / "keep").write_text("kept\n", encoding="utf-8")
    subprocess.run(["git", "add", "tests/fixtures/keep"], cwd=project_dir, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.email=canary@example.invalid",
            "-c",
            "user.name=canary",
            "commit",
            "-q",
            "-m",
            "fixtures",
        ],
        cwd=project_dir,
        check=True,
    )


def _install_detector(pytester: pytest.Pytester) -> None:
    """Copy the real root conftest.py into the pytester project root."""
    pytester.makepyfile(conftest=_ROOT_CONFTEST.read_text(encoding="utf-8"))


def _run_nested(pytester: pytest.Pytester) -> pytest.RunResult:
    basetemp = pytester.path / "basetemp"
    return pytester.run(
        sys.executable,
        "-mpytest",
        "-q",
        "-p",
        "no:cacheprovider",
        "--basetemp",
        str(basetemp),
    )


@pytest.mark.timeout(30)
def test_detector_fires_on_leaked_file(pytester: pytest.Pytester) -> None:
    _install_detector(pytester)
    _prime_git_repo(pytester.path)
    pytester.makepyfile(
        test_leak="""
        from pathlib import Path

        def test_writes_into_fixtures(tmp_path):
            Path("tests/fixtures/leak_probe.tmp").write_text("x")
        """
    )
    result = _run_nested(pytester)
    assert result.ret != 0, result.stderr.str() + result.stdout.str()
    combined = result.stderr.str() + result.stdout.str()
    assert "solstone fixture-leak detector" in combined
    assert "tests/fixtures/leak_probe.tmp" in combined
    assert "journal_copy fixture" in combined


@pytest.mark.timeout(30)
def test_detector_silent_on_clean(pytester: pytest.Pytester) -> None:
    _install_detector(pytester)
    _prime_git_repo(pytester.path)
    pytester.makepyfile(
        test_clean="""
        def test_noop():
            assert True
        """
    )
    result = _run_nested(pytester)
    assert result.ret == 0, result.stderr.str() + result.stdout.str()
    combined = result.stderr.str() + result.stdout.str()
    assert "fixture-leak detector" not in combined


@pytest.mark.timeout(30)
def test_detector_skips_without_git_repo(pytester: pytest.Pytester) -> None:
    _install_detector(pytester)
    pytester.makepyfile(
        test_clean="""
        def test_noop():
            assert True
        """
    )
    result = _run_nested(pytester)
    assert result.ret == 0, result.stderr.str() + result.stdout.str()
    combined = result.stderr.str() + result.stdout.str()
    assert "git unavailable" in combined
