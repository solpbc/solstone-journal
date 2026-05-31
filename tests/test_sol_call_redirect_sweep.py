# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

EXCLUDED_DIRS = {".git", ".venv", "build", "dist"}
TEXT_SUFFIXES = {
    ".html",
    ".json",
    ".md",
    ".py",
    ".sh",
    ".toml",
    ".txt",
    ".yml",
    ".yaml",
}
ROOT_TEXT_FILES = {
    "AGENTS.md",
    "CONTRIBUTING.md",
    "INSTALL.md",
    "Makefile",
    "README.md",
}
REQUIRED_TRACKED_PATHS = {
    Path("journal/AGENTS.md"),
    Path("tests/fixtures/journal/AGENTS.md"),
    Path("tests/fixtures/journal/identity/partner.md"),
    Path("tests/baselines/api/settings/providers.json"),
}
BAN_PATTERNS = (
    re.compile(r"sol call " r"(navigate|routines|identity)\b"),
    re.compile(r"sol call settings providers " r"install"),
)


def _tracked_files() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files"],
        check=True,
        capture_output=True,
        text=True,
    )
    return [Path(line) for line in result.stdout.splitlines() if line.strip()]


def _is_text_surface(path: Path) -> bool:
    if any(part in EXCLUDED_DIRS for part in path.parts):
        return False
    if path.name == "CHANGELOG.md":
        return False
    if path.name in ROOT_TEXT_FILES:
        return True
    return path.suffix in TEXT_SUFFIXES


def _candidate_files() -> list[Path]:
    files = sorted(path for path in _tracked_files() if _is_text_surface(path))
    missing = REQUIRED_TRACKED_PATHS - set(files)
    assert missing == set()
    return files


@pytest.mark.parametrize("path", _candidate_files(), ids=str)
def test_old_sol_call_local_paths_are_not_documented(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    matches = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        for pattern in BAN_PATTERNS:
            if pattern.search(line):
                matches.append((line_number, line))

    assert matches == []


@pytest.mark.parametrize(
    "survivor",
    [
        "sol call settings identity",
        "sol call settings providers show",
        "sol call settings providers set-generate",
        "sol call settings providers set-cogitate",
    ],
)
def test_old_sol_call_ban_regex_precision(survivor: str) -> None:
    assert all(pattern.search(survivor) is None for pattern in BAN_PATTERNS)
