# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

import pytest

from solstone.think.sol_cli import ALIASES, COMMANDS

EXCLUDED_DIRS = {
    ".git",
    ".venv",
    "build",
    "dist",
    "htmlcov",
    "journal",
    "logs",
    "node_modules",
    "scratch",
    "tmp",
    "vpe",
}
TEXT_SUFFIXES = {
    ".html",
    ".js",
    ".md",
    ".py",
    ".rules",
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
ACCESS_POSITIVE_EXPECTATIONS = {
    Path("AGENTS.md"): re.compile(r"\bsol call\b"),
    Path("INSTALL.md"): re.compile(r"\bsol doctor\b"),
    Path("Makefile"): re.compile(r"\$\(VENV_BIN\)/sol skills\b"),
    Path("README.md"): re.compile(r"\bsol chat\b"),
}


def _is_text_surface(path: Path) -> bool:
    if any(part in EXCLUDED_DIRS for part in path.parts):
        return False
    if path.name == "CHANGELOG.md":
        return False
    if path.name in ROOT_TEXT_FILES:
        return True
    return path.suffix in TEXT_SUFFIXES


def _candidate_files() -> list[Path]:
    return sorted(
        path
        for path in Path(".").rglob("*")
        if path.is_file() and _is_text_surface(path)
    )


def _skip_line(path: Path, line: str) -> bool:
    if path in {
        Path("tests/test_cli_prog_fidelity.py"),
        Path("tests/test_journal_cli_migration.py"),
    }:
        return True
    if (
        path == Path("tests/test_install_guard.py")
        and "managed by 'sol config'" in line
    ):
        return True
    if path.parts and path.parts[0] == "tests":
        argv_markers = (
            "sys.argv",
            '["sol ',
            "usage: sol ",
            "run_main(mod,",
            "sol think.talents",
        )
        if any(marker in line for marker in argv_markers):
            return True
        if re.search(
            r'"sol (' + "|".join(map(re.escape, SERVICE_TERMS)) + r")\b", line
        ):
            return True
    return False


SERVICE_TERMS = sorted(
    {
        *(name for name, command in COMMANDS.items() if command.surface == "service"),
        *(name for name, alias in ALIASES.items() if alias.surface == "service"),
    },
    key=len,
    reverse=True,
)
SERVICE_SOL_RE = re.compile(
    r"\bsol (" + "|".join(map(re.escape, SERVICE_TERMS)) + r")\b"
)
SERVICE_SOL_LITERAL_RE = re.compile(
    r"""['"]sol['"]\s*,\s*['"](?:"""
    + "|".join(map(re.escape, SERVICE_TERMS))
    + r""")['"]"""
)


@pytest.mark.parametrize("path", _candidate_files(), ids=str)
def test_service_tagged_commands_are_not_documented_as_sol(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    checked_lines = [line for line in text.splitlines() if not _skip_line(path, line)]
    service_matches = [
        (line_number, line)
        for line_number, line in enumerate(checked_lines, start=1)
        if SERVICE_SOL_RE.search(line)
    ]

    assert service_matches == []

    access_expectation = ACCESS_POSITIVE_EXPECTATIONS.get(path)
    if access_expectation is not None:
        assert access_expectation.search(text)


def _production_python_files() -> list[Path]:
    return sorted(
        path
        for path in Path("solstone").rglob("*.py")
        if path.is_file() and "tests" not in path.parts
    )


def test_production_service_commands_do_not_dispatch_through_sol() -> None:
    matches = []
    for path in _production_python_files():
        text = path.read_text(encoding="utf-8")
        for match in SERVICE_SOL_LITERAL_RE.finditer(text):
            line_number = text.count("\n", 0, match.start()) + 1
            matches.append(f"{path}:{line_number}: {match.group(0)!r}")

    assert matches == []
