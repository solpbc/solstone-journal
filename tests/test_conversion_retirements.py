# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""The retirement check must describe the repository, not one operator's disk."""

from __future__ import annotations

import subprocess
from pathlib import Path

import scripts.check_conversion_retirements as checker

MANIFEST = """\
schema_version = 2
dependency_files = ["pyproject.toml"]
content_roots = ["src"]
content_exclusions = []

[[waves]]
status = "done"
distribution = "retired-dist"
python_roots = ["src/legacy"]
import_roots = ["retired_dist"]
test_only_dependency_locations = []
"""


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=t@e", "-c", "user.name=t", *args],
        cwd=repo,
        check=True,
    )


def _repo(base: Path) -> Path:
    base.mkdir(parents=True, exist_ok=True)
    _git(base, "init", "-q")
    (base / "pyproject.toml").write_text('[project]\nname = "x"\n', encoding="utf-8")
    (base / ".gitignore").write_text("__pycache__/\n", encoding="utf-8")
    (base / "src").mkdir()
    (base / "src" / "keep.py").write_text("x = 1\n", encoding="utf-8")
    _git(base, "add", "-A")
    _git(base, "commit", "-qm", "base")
    return base


def _run(repo: Path, tmp_path: Path):
    manifest = tmp_path / "conversion-retirements.toml"
    manifest.write_text(MANIFEST, encoding="utf-8")
    return checker.check_repository(repo, manifest)


def test_retired_root_absent_passes(tmp_path):
    repo = _repo(tmp_path / "r")

    result = _run(repo, tmp_path)

    assert result.ok, result.violations


def test_retired_root_holding_only_ignored_build_output_passes(tmp_path):
    """A checkout that removed the sources leaves __pycache__; the repo did not."""
    repo = _repo(tmp_path / "r")
    cache = repo / "src" / "legacy" / "__pycache__"
    cache.mkdir(parents=True)
    (cache / "mod.cpython-313.pyc").write_bytes(b"\x00")

    result = _run(repo, tmp_path)

    assert result.ok, result.violations


def test_retired_root_with_tracked_source_still_fails(tmp_path):
    repo = _repo(tmp_path / "r")
    legacy = repo / "src" / "legacy"
    legacy.mkdir(parents=True)
    (legacy / "mod.py").write_text("x = 1\n", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "legacy")

    result = _run(repo, tmp_path)

    assert not result.ok
    assert any("src/legacy" in item for item in result.violations), result.violations


def test_retired_root_with_untracked_unignored_source_still_fails(tmp_path):
    """Not a weakening: only ignored content is forgiven."""
    repo = _repo(tmp_path / "r")
    legacy = repo / "src" / "legacy"
    legacy.mkdir(parents=True)
    (legacy / "stray.py").write_text("x = 1\n", encoding="utf-8")

    result = _run(repo, tmp_path)

    assert not result.ok
    assert any("src/legacy" in item for item in result.violations), result.violations
